use std::io::{BufRead, BufReader};
use std::sync::mpsc::Sender;

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::agent::WorkerEvent;
use crate::api::{ModelTurnRequest, ModelTurnResponse, ModelTurnTransport, ToolCall};
use crate::auth::CodexAuth;

#[derive(Debug, Clone)]
pub struct CodexClient {
    client: reqwest::blocking::Client,
}

const LEGACY_RESPONSES_API_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

impl CodexClient {
    pub fn new() -> Result<Self> {
        let client = reqwest::blocking::Client::builder().timeout(None).build()?;
        Ok(Self { client })
    }
}

impl ModelTurnTransport for CodexClient {
    fn execute_turn(
        &self,
        auth: &CodexAuth,
        request: &ModelTurnRequest,
        session_id: String,
        turn_id: String,
        tx: &Sender<WorkerEvent>,
    ) -> Result<ModelTurnResponse> {
        let mut tool_calls = Vec::new();
        let mut text_acc = String::new();
        let mut last_event = None;

        let mut req = self
            .client
            .post(LEGACY_RESPONSES_API_URL)
            .bearer_auth(&auth.access_token)
            .header("Accept", "text/event-stream")
            .header("OpenAI-Beta", "responses=experimental");

        if let Some(ref acct) = auth.account_id {
            req = req.header("ChatGPT-Account-ID", acct);
        }

        let response = req.json(&request.body).send()?;

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
                    let done = dispatch_sse_event(
                        &data,
                        session_id.clone(),
                        turn_id.clone(),
                        request.emit_text_events,
                        tx,
                        &mut tool_calls,
                        &mut text_acc,
                        &mut last_event,
                    )
                    .map_err(|e| anyhow!(e))?;
                    data.clear();
                    if done {
                        break;
                    }
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("data:") {
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                if !data.is_empty() {
                    if rest.trim().starts_with('{') {
                        let done = dispatch_sse_event(
                            &data,
                            session_id.clone(),
                            turn_id.clone(),
                            request.emit_text_events,
                            tx,
                            &mut tool_calls,
                            &mut text_acc,
                            &mut last_event,
                        )
                        .map_err(|e| anyhow!(e))?;
                        data.clear();
                        if done {
                            break;
                        }
                    } else {
                        data.push('\n');
                    }
                }
                data.push_str(rest);
            }
        }

        if !data.is_empty() {
            dispatch_sse_event(
                &data,
                session_id,
                turn_id,
                request.emit_text_events,
                tx,
                &mut tool_calls,
                &mut text_acc,
                &mut last_event,
            )
            .ok();
        }

        Ok(ModelTurnResponse {
            text: text_acc,
            tool_calls,
            finish_reason: None,
            raw_payload: last_event,
        })
    }
}

fn find_tool_calls(v: &Value, calls: &mut Vec<ToolCall>) {
    // Legacy Responses payloads may nest tool calls in several places, but they
    // are always normalized into the transport-neutral `ToolCall` struct here
    // before the provider loop sees them.
    if let Some(obj) = v.as_object() {
        if obj.get("type").and_then(|t| t.as_str()) == Some("function_call") {
            let v_obj = Value::Object(obj.clone());
            let id = obj
                .get("call_id")
                .or_else(|| obj.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let name = obj
                .get("name")
                .or_else(|| v_obj.pointer("/function/name"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let args_val = obj
                .get("arguments")
                .or_else(|| v_obj.pointer("/function/arguments"))
                .cloned()
                .unwrap_or_default();
            let args: Value = if let Some(s) = args_val.as_str() {
                serde_json::from_str(s).unwrap_or_default()
            } else {
                args_val
            };
            if !name.is_empty() {
                calls.push(ToolCall::new(id, name, args));
            }
        }
        for value in obj.values() {
            find_tool_calls(value, calls);
        }
    } else if let Some(arr) = v.as_array() {
        for value in arr {
            find_tool_calls(value, calls);
        }
    }
}

fn dispatch_sse_event(
    data: &str,
    session_id: String,
    turn_id: String,
    emit_text_events: bool,
    tx: &Sender<WorkerEvent>,
    tool_calls: &mut Vec<ToolCall>,
    text_acc: &mut String,
    last_event: &mut Option<Value>,
) -> std::result::Result<bool, String> {
    // Legacy Responses SSE parser. It expects event names like
    // `response.output_text.delta` and recursively scans payloads for
    // `function_call` objects, both of which will need replacement in step 7.
    if data.trim() == "[DONE]" {
        return Ok(true);
    }
    let value: Value = serde_json::from_str(data).map_err(|e| format!("parse sse event: {e}"))?;
    *last_event = Some(value.clone());

    let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");

    find_tool_calls(&value, tool_calls);

    match kind {
        "response.output_text.delta" | "response.text.delta" | "text.delta" => {
            if let Some(delta) = value.get("delta").and_then(|v| v.as_str()) {
                text_acc.push_str(delta);
                if emit_text_events {
                    let _ = tx.send(WorkerEvent::Delta {
                        session_id,
                        turn_id,
                        delta: delta.to_string(),
                    });
                }
            }
        }
        "output" => {
            if let Some(content_arr) = value.pointer("/content").and_then(|v| v.as_array()) {
                for item in content_arr {
                    if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                            if text_acc.is_empty() {
                                text_acc.push_str(text);
                                if emit_text_events {
                                    let _ = tx.send(WorkerEvent::Delta {
                                        session_id: session_id.clone(),
                                        turn_id: turn_id.clone(),
                                        delta: text.to_string(),
                                    });
                                }
                            }
                        }
                    }
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
    Ok(false)
}
