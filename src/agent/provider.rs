use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use anyhow::Result;

use crate::agent::{ProviderRequest, WorkerEvent, ProviderMessage};
use crate::api::codex::CodexClient;
use crate::auth::CodexAuth;
use crate::models::Role;
use crate::tools::{self, Sandbox};

pub trait Provider: std::fmt::Debug + Send + 'static {
    fn generate(&self, request: &ProviderRequest, session_id: u64, tx: &Sender<WorkerEvent>);
    fn label(&self) -> &'static str;
}

#[derive(Debug)]
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

#[derive(Debug)]
pub struct CodexProvider {
    auth: CodexAuth,
    client: CodexClient,
    sandbox: Sandbox,
}

impl CodexProvider {
    pub fn new(auth: CodexAuth, workspace: PathBuf) -> Result<Self> {
        let client = CodexClient::new()?;
        let sandbox = Sandbox::new(workspace)?;
        Ok(Self {
            auth,
            client,
            sandbox,
        })
    }
}

impl Provider for CodexProvider {
    fn label(&self) -> &'static str {
        "codex"
    }

    fn generate(&self, request: &ProviderRequest, session_id: u64, tx: &Sender<WorkerEvent>) {
        let mut messages = request.messages.clone();

        for _ in 0..8 {
            let input: Vec<serde_json::Value> = messages
                .iter()
                .map(|m| m.as_json())
                .collect();

            let mut instructions = String::new();
            if let Some(ref board) = request.board {
                instructions = format!("Current Project Board State:\n{}", serde_yaml::to_string(board).unwrap_or_default());
            }

            let body = serde_json::json!({
                "model": request.model,
                "instructions": instructions,
                "input": input,
                "tools": tools::tool_specs(),
                "tool_choice": "auto",
                "parallel_tool_calls": false,
                "reasoning": null,
                "store": false,
                "stream": true,
            });

            let tool_calls = match self.client.request(&self.auth, &body, session_id, tx) {
                Ok(tc) => tc,
                Err(e) => {
                    let _ = tx.send(WorkerEvent::Error {
                        session_id,
                        err: format!("{}", e),
                    });
                    return;
                }
            };

            if tool_calls.is_empty() {
                let _ = tx.send(WorkerEvent::Done { session_id });
                return;
            }

            let mut tool_outputs = Vec::new();
            for call in tool_calls {
                let slug = tools::tool_slug(&call.name, &call.args);
                let _ = tx.send(WorkerEvent::SystemNote {
                    session_id,
                    note: slug,
                });
                let _ = tx.send(WorkerEvent::ToolStatus {
                    session_id,
                    status: "running tools...".into(),
                });

                let result = tools::execute_tool(&self.sandbox, &call.name, &call.args);

                if call.name == "board_update" {
                    let _ = tx.send(WorkerEvent::BoardUpdate {
                        board: call.args.clone(),
                    });
                }

                let output = match result {
                    Ok(output) => output,
                    Err(e) => format!("[error] {}", e),
                };
                tool_outputs.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": call.id,
                    "output": output,
                }));
            }

            messages.push(ProviderMessage {
                role: Role::ToolResult,
                content: serde_json::to_string(&tool_outputs).unwrap(),
            });
            messages.push(ProviderMessage {
                role: Role::Assistant,
                content: String::new(),
            });
        }

        let _ = tx.send(WorkerEvent::Error {
            session_id,
            err: "too many tool-call iterations".into(),
        });
    }
}
