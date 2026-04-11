use std::fs;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct AuthFile {
    tokens: Option<Tokens>,
}

#[derive(Debug, Deserialize)]
struct Tokens {
    access_token: String,
    #[serde(default)]
    account_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CodexAuth {
    pub access_token: String,
    pub account_id: Option<String>,
}

impl CodexAuth {
    pub fn load_from_disk() -> Result<Self> {
        let path = dirs::home_dir()
            .context("no home directory")?
            .join(".codex/auth.json");
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let parsed: AuthFile = serde_json::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        let tokens = parsed
            .tokens
            .ok_or_else(|| anyhow!("no `tokens` in {}", path.display()))?;
        Ok(Self {
            access_token: tokens.access_token,
            account_id: tokens.account_id,
        })
    }
}
