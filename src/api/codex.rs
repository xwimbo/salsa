use std::io::{BufRead, BufReader};
use std::sync::mpsc::Sender;

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::agent::WorkerEvent;
use crate::auth::CodexAuth;

#[derive(Debug)]
pub struct CodexClient {
    client: reqwest::blocking::Client,
}

#[derive(Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

impl CodexClient {
    pub fn new() -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(None)
            .build()?;
        Ok(Self { client })
    }

    pub fn request(
        &self,
        auth: &CodexAuth,
        body: &Value,
        session_id: u64,
        tx: &Sender<WorkerEvent>,
    ) -> Result<Vec<ToolCall>> {
        let mut tool_calls = Vec::new();

        let mut req = self
            .client
            .post("https://chatgpt.com/backend-api/codex/responses")
            .bearer_auth(&auth.access_token)
            .header("Accept", "text/event-stream")
            .header("OpenAI-Beta", "responses=experimental");
        
        if let Some(ref acct) = auth.account_id {
            req = req.header("ChatGPT-Account-ID", acct);
        }

        let response = req.json(body).send()?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            let truncated: String = body.chars().take(500).collect();
            return Err(anyhow!("http {status}: {truncated}"));
        }

        let reader = BufReader::new(response);
        let mut data = String::new();
        for line in reader.lines() {
            let line = line?;
            if line.is_empty() {
                if !data.is_empty() {
                    dispatch_sse_event(&data, session_id, tx, &mut tool_calls)
                        .map_err(|e| anyhow!(e))?;
                    data.clear();
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("data:") {
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest);
            }
        }

        Ok(tool_calls)
    }
}

fn dispatch_sse_event(
    data: &str,
    session_id: u64,
    tx: &Sender<WorkerEvent>,
    tool_calls: &mut Vec<ToolCall>,
) -> std::result::Result<(), String> {
    if data == "[DONE]" {
        return Ok(());
    }
    let value: Value =
        serde_json::from_str(data).map_err(|e| format!("parse sse event: {e}"))?;
    let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        "response.output_text.delta" => {
            if let Some(delta) = value.get("delta").and_then(|v| v.as_str()) {
                let _ = tx.send(WorkerEvent::Delta {
                    session_id,
                    delta: delta.to_string(),
                });
            }
        }
        "response.message.tool_calls" => {
            if let Some(calls) = value.get("calls").and_then(|v| v.as_array()) {
                for call in calls {
                    let id = call
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let name = call
                        .pointer("/function/name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let args: Value = call
                        .pointer("/function/arguments")
                        .and_then(|v| serde_json::from_str(v.as_str().unwrap_or("{}")).ok())
                        .unwrap_or_default();
                    tool_calls.push(ToolCall { id, name, args });
                }
            }
        }
        "response.failed" | "error" => {
            let err = value
                .pointer("/response/error/message")
                .or_else(|| value.pointer("/error/message"))
                .or_else(|| value.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
                .to_string();
            return Err(err);
        }
        _ => {}
    }
    Ok(())
}
