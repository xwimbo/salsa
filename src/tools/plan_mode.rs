use anyhow::Result;
use serde_json::{json, Value};

use crate::models::BoardOperation;

pub fn execute_enter(_args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    Ok((
        "Entered plan mode. Tool access is now restricted to read-only operations. \
         Use exit_plan_mode when ready to implement."
            .to_string(),
        Vec::new(),
    ))
}

pub fn execute_exit(args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let summary = args
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("No summary provided.");
    Ok((
        format!(
            "Exited plan mode. Full tool access restored.\nPlan summary: {}",
            summary
        ),
        Vec::new(),
    ))
}

pub fn enter_spec() -> Value {
    json!({
        "type": "function",
        "name": "enter_plan_mode",
        "description": "Switch to plan mode. In plan mode only read-only tools are available. Use this to explore and design before making changes.",
        "parameters": {
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }
    })
}

pub fn exit_spec() -> Value {
    json!({
        "type": "function",
        "name": "exit_plan_mode",
        "description": "Exit plan mode and restore full tool access. Provide a summary of the plan.",
        "parameters": {
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "A brief summary of the plan before switching to implementation."
                }
            },
            "additionalProperties": false
        }
    })
}
