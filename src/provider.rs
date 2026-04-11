use std::io::{BufRead, BufReader};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use anyhow::Result;

use crate::auth::CodexAuth;
use crate::Role;

#[derive(Debug, Clone)]
pub struct ProviderMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ProviderRequest {
    pub messages: Vec<ProviderMessage>,
    #[allow(dead_code)]
    pub model: String,
}

#[derive(Debug)]
pub enum WorkerCmd {
    Send {
        session_id: u64,
        request: ProviderRequest,
    },
    #[allow(dead_code)]
    Shutdown,
}

#[derive(Debug)]
pub enum WorkerEvent {
    Delta { session_id: u64, delta: String },
    Done { session_id: u64 },
    #[allow(dead_code)]
    Error { session_id: u64, err: String },
}

pub trait Provider: Send + 'static {
    fn generate(&self, request: &ProviderRequest, session_id: u64, tx: &Sender<WorkerEvent>);
    fn label(&self) -> &'static str;
}

pub struct EchoProvider;

impl Provider for EchoProvider {
    fn label(&self) -> &'static str {
        "echo"
    }

    fn generate(&self, request: &ProviderRequest, session_id: u64, tx: &Sender<WorkerEvent>) {
        let last_user = request
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, Role::User))
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let reply = if last_user.trim().is_empty() {
            "(echo provider) — empty input.".to_string()
        } else {
            format!(
                "You said: \"{}\". That's {} characters. The real provider arrives in phase 2 of step (v).",
                last_user.trim(),
                last_user.chars().count()
            )
        };

        // Stream word-by-word so the UI can render deltas live.
        for chunk in reply.split_inclusive(' ') {
            thread::sleep(Duration::from_millis(35));
            if tx
                .send(WorkerEvent::Delta {
                    session_id,
                    delta: chunk.to_string(),
                })
                .is_err()
            {
                return;
            }
        }

        let _ = tx.send(WorkerEvent::Done { session_id });
    }
}

pub struct CodexProvider {
    auth: CodexAuth,
    client: reqwest::blocking::Client,
}

impl CodexProvider {
    pub fn new(auth: CodexAuth) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(None)
            .build()?;
        Ok(Self { auth, client })
    }
}

impl Provider for CodexProvider {
    fn label(&self) -> &'static str {
        "codex"
    }

    fn generate(&self, request: &ProviderRequest, session_id: u64, tx: &Sender<WorkerEvent>) {
        let input: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|m| {
                let (role, content_type) = match m.role {
                    Role::User => ("user", "input_text"),
                    Role::Assistant => ("assistant", "output_text"),
                };
                serde_json::json!({
                    "type": "message",
                    "role": role,
                    "content": [{"type": content_type, "text": m.content}],
                })
            })
            .collect();

        let body = serde_json::json!({
            "model": request.model,
            "instructions": "",
            "input": input,
            "tools": [],
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "reasoning": null,
            "store": false,
            "stream": true,
            "include": [],
        });

        let mut req = self
            .client
            .post("https://chatgpt.com/backend-api/codex/responses")
            .bearer_auth(&self.auth.access_token)
            .header("Accept", "text/event-stream")
            .header("OpenAI-Beta", "responses=experimental");
        if let Some(ref acct) = self.auth.account_id {
            req = req.header("ChatGPT-Account-ID", acct);
        }

        let response = match req.json(&body).send() {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(WorkerEvent::Error {
                    session_id,
                    err: format!("request failed: {e}"),
                });
                return;
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            let truncated: String = body.chars().take(500).collect();
            let _ = tx.send(WorkerEvent::Error {
                session_id,
                err: format!("http {status}: {truncated}"),
            });
            return;
        }

        let reader = BufReader::new(response);
        let mut data = String::new();
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    let _ = tx.send(WorkerEvent::Error {
                        session_id,
                        err: format!("stream read: {e}"),
                    });
                    return;
                }
            };
            if line.is_empty() {
                if !data.is_empty() {
                    if let Err(err) = dispatch_sse_event(&data, session_id, tx) {
                        let _ = tx.send(WorkerEvent::Error { session_id, err });
                        return;
                    }
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
            // Other SSE fields (event:, id:, retry:) are ignored.
        }
        // In case the stream terminated without an explicit response.completed.
        let _ = tx.send(WorkerEvent::Done { session_id });
    }
}

fn dispatch_sse_event(
    data: &str,
    session_id: u64,
    tx: &Sender<WorkerEvent>,
) -> std::result::Result<(), String> {
    if data == "[DONE]" {
        return Ok(());
    }
    let value: serde_json::Value = serde_json::from_str(data)
        .map_err(|e| format!("parse sse event: {e}"))?;
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
        "response.completed" => {
            let _ = tx.send(WorkerEvent::Done { session_id });
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

pub struct WorkerHandles {
    pub cmd_tx: Sender<WorkerCmd>,
    pub event_rx: Receiver<WorkerEvent>,
    pub provider_label: &'static str,
}

pub fn spawn_worker(provider: Box<dyn Provider>) -> WorkerHandles {
    let provider_label = provider.label();
    let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
    let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>();
    thread::spawn(move || {
        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                WorkerCmd::Send {
                    session_id,
                    request,
                } => {
                    provider.generate(&request, session_id, &event_tx);
                }
                WorkerCmd::Shutdown => break,
            }
        }
    });
    WorkerHandles {
        cmd_tx,
        event_rx,
        provider_label,
    }
}
