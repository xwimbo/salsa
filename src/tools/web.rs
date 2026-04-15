use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::models::BoardOperation;

const CACHE_TTL_SECS: u64 = 15 * 60;
const MAX_OUTPUT_CHARS: usize = 100_000;
const FETCH_TIMEOUT_SECS: u64 = 30;

fn cache_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".salsa")
        .join("web_cache")
}

fn url_hash(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn load_cached(url: &str) -> Option<String> {
    let path = cache_dir().join(format!("{}.txt", url_hash(url)));
    if !path.exists() {
        return None;
    }
    let meta = fs::metadata(&path).ok()?;
    let modified = meta.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?;
    if age.as_secs() > CACHE_TTL_SECS {
        return None;
    }
    fs::read_to_string(&path).ok()
}

fn save_cached(url: &str, content: &str) {
    let dir = cache_dir();
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(format!("{}.txt", url_hash(url)));
    let _ = fs::write(path, content);
}

fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut last_was_whitespace = false;

    let lower = html.to_lowercase();
    let chars: Vec<char> = html.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if !in_tag {
            if chars[i] == '<' {
                in_tag = true;
                // Check for script/style open tags
                let remaining: String = lower_chars[i..].iter().take(10).collect();
                if remaining.starts_with("<script") {
                    in_script = true;
                } else if remaining.starts_with("<style") {
                    in_style = true;
                }
                // Block-level tags get newlines
                let block_tags = [
                    "<p", "<div", "<br", "<h1", "<h2", "<h3", "<h4", "<h5", "<h6",
                    "<li", "<tr", "<blockquote", "<pre", "<hr",
                ];
                for bt in &block_tags {
                    if remaining.starts_with(bt) {
                        if !out.ends_with('\n') && !out.is_empty() {
                            out.push('\n');
                        }
                        break;
                    }
                }
                i += 1;
                continue;
            }

            if in_script || in_style {
                i += 1;
                continue;
            }

            // Decode basic HTML entities
            if chars[i] == '&' {
                let rest: String = chars[i..].iter().take(10).collect();
                if rest.starts_with("&amp;") {
                    out.push('&');
                    i += 5;
                    last_was_whitespace = false;
                    continue;
                } else if rest.starts_with("&lt;") {
                    out.push('<');
                    i += 4;
                    last_was_whitespace = false;
                    continue;
                } else if rest.starts_with("&gt;") {
                    out.push('>');
                    i += 4;
                    last_was_whitespace = false;
                    continue;
                } else if rest.starts_with("&quot;") {
                    out.push('"');
                    i += 6;
                    last_was_whitespace = false;
                    continue;
                } else if rest.starts_with("&nbsp;") {
                    out.push(' ');
                    i += 6;
                    last_was_whitespace = true;
                    continue;
                } else if rest.starts_with("&#") {
                    // Numeric entity
                    if let Some(semi) = rest.find(';') {
                        let num_str = &rest[2..semi];
                        let codepoint = if let Some(hex) = num_str.strip_prefix('x') {
                            u32::from_str_radix(hex, 16).ok()
                        } else {
                            num_str.parse::<u32>().ok()
                        };
                        if let Some(cp) = codepoint {
                            if let Some(ch) = char::from_u32(cp) {
                                out.push(ch);
                                i += semi + 1;
                                last_was_whitespace = false;
                                continue;
                            }
                        }
                    }
                }
            }

            let ch = chars[i];
            if ch.is_whitespace() {
                if !last_was_whitespace && !out.is_empty() {
                    out.push(' ');
                    last_was_whitespace = true;
                }
            } else {
                out.push(ch);
                last_was_whitespace = false;
            }
        } else {
            // Inside a tag
            if chars[i] == '>' {
                in_tag = false;
                let remaining: String = lower_chars[i.saturating_sub(8)..=i].iter().collect();
                if remaining.contains("/script>") {
                    in_script = false;
                } else if remaining.contains("/style>") {
                    in_style = false;
                }
            }
        }
        i += 1;
    }

    // Collapse multiple blank lines
    let mut result = String::new();
    let mut blank_count = 0u32;
    for line in out.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                result.push('\n');
            }
        } else {
            blank_count = 0;
            result.push_str(trimmed);
            result.push('\n');
        }
    }

    result.trim().to_string()
}

pub fn execute(args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `url`"))?;

    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(anyhow!("url must start with http:// or https://"));
    }

    // Check cache
    if let Some(cached) = load_cached(url) {
        return Ok((cached, Vec::new()));
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| anyhow!("failed to build HTTP client: {}", e))?;

    let response = client
        .get(url)
        .header("User-Agent", "salsa/0.1")
        .send()
        .map_err(|e| anyhow!("fetch failed: {}", e))?;

    if !response.status().is_success() {
        return Err(anyhow!("HTTP {}", response.status()));
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let body = response
        .text()
        .map_err(|e| anyhow!("failed to read body: {}", e))?;

    let text = if content_type.contains("html") {
        strip_html(&body)
    } else {
        body
    };

    let mut output = text;
    if output.len() > MAX_OUTPUT_CHARS {
        output.truncate(MAX_OUTPUT_CHARS);
        output.push_str("\n[truncated]");
    }

    save_cached(url, &output);

    Ok((output, Vec::new()))
}

pub fn spec() -> Value {
    json!({
        "type": "function",
        "name": "web_fetch",
        "description": "Fetch a URL and return its text content. HTML is automatically converted to plain text. Results are cached for 15 minutes.",
        "parameters": {
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch (must start with http:// or https://)."
                }
            },
            "required": ["url"],
            "additionalProperties": false
        }
    })
}
