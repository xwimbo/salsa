use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use serde_json;
use serde_yaml;

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
                .filter(|m| !matches!(m.role, Role::System))
                .map(|m| m.as_json())
                .collect();

            let mut instructions = String::from(
                "You are an expert software engineer assistant with access to a sandboxed workspace.\n\
                You must use your provided tools to interact with the file system or run commands.\n\
                \n\
                Available tools:\n\
                - fs_read: Read a file\n\
                - fs_write: Write/create a file\n\
                - fs_list: List files in a directory\n\
                - fs_edit: Edit a file (search and replace)\n\
                - fs_delete: Delete a file\n\
                - sh_run: Run a shell command\n\
                - board_update: Update the project board state\n\
                \n\
                CRITICAL: Never lie about using a tool. If you need to perform an action, you MUST call the tool.\n\
                Do not claim to have performed an action unless you have actually called the tool and received a successful result.\n\
                Be concise, honest, and direct in your responses."
            );
            if let Some(ref board) = request.board {
                instructions.push_str(&format!("\n\nCurrent Project Board State:\n{}", serde_yaml::to_string(board).unwrap_or_default()));
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

            let (text_content, tool_calls) = match self.client.request(&self.auth, &body, session_id, tx) {
                Ok(res) => res,
                Err(e) => {
                    let _ = tx.send(WorkerEvent::Error {
                        session_id,
                        err: format!("{}", e),
                    });
                    return;
                }
            };

            // Deduplicate tool calls by ID, prioritizing those with non-null arguments
            let mut calls_map: std::collections::HashMap<String, crate::api::codex::ToolCall> = std::collections::HashMap::new();
            for tc in tool_calls {
                let is_empty = tc.args.is_null() || (tc.args.is_object() && tc.args.as_object().unwrap().is_empty());
                if !is_empty || !calls_map.contains_key(&tc.id) {
                    calls_map.insert(tc.id.clone(), tc);
                }
            }
            let unique_calls: Vec<_> = calls_map.into_values().collect();

            if unique_calls.is_empty() {
                // We've already emitted deltas during request(), so we just finish.
                let _ = tx.send(WorkerEvent::Done { session_id });
                return;
            }

            let calls_json = serde_json::json!(unique_calls.iter().map(|tc| {
                serde_json::json!({
                    "id": tc.id,
                    "function": {
                        "name": tc.name,
                        "arguments": tc.args
                    }
                })
            }).collect::<Vec<_>>());

            let _ = tx.send(WorkerEvent::ToolCalls {
                session_id,
                calls: calls_json.clone(),
            });

            // Update history with the assistant's response (text + tool calls)
            messages.push(ProviderMessage {
                role: Role::Assistant,
                content: text_content,
                tool_calls: Some(calls_json),
            });

            let mut tool_outputs = Vec::new();
            for call in unique_calls {
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

            let result_content = serde_json::to_string(&tool_outputs).unwrap();
            let _ = tx.send(WorkerEvent::ToolResult {
                session_id,
                content: result_content.clone(),
            });

            messages.push(ProviderMessage {
                role: Role::ToolResult,
                content: result_content,
                tool_calls: None,
            });
        }

        let _ = tx.send(WorkerEvent::Error {
            session_id,
            err: "too many tool-call iterations".into(),
        });
    }
}
