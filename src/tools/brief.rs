use std::fs;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use super::sandbox::Sandbox;
use crate::models::BoardOperation;

pub fn execute(sandbox: &Sandbox, args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let message = args
        .get("message")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `message`"))?;

    let mut parts = vec![message.to_string()];

    if let Some(attachments) = args.get("attachments").and_then(|v| v.as_array()) {
        for att in attachments {
            if let Some(path) = att.as_str() {
                let abs = sandbox.resolve(path)?;
                let meta = fs::metadata(&abs)
                    .map_err(|e| anyhow!("attachment `{}`: {}", path, e))?;
                parts.push(format!(
                    "[attachment: {} ({} bytes)]",
                    path,
                    meta.len()
                ));
            }
        }
    }

    Ok((parts.join("\n"), Vec::new()))
}

pub fn spec() -> Value {
    json!({
        "type": "function",
        "name": "brief",
        "description": "Send a formatted message to the user, optionally referencing workspace files as attachments.",
        "parameters": {
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The message content to display to the user."
                },
                "attachments": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of workspace-relative file paths to attach."
                }
            },
            "required": ["message"],
            "additionalProperties": false
        }
    })
}
