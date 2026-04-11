pub mod provider;
pub mod worker;

pub use provider::Provider;
pub use worker::WorkerHandles;

use crate::models::Role;
use serde_json;

#[derive(Debug, Clone)]
pub struct ProviderMessage {
    pub role: Role,
    pub content: String,
    pub tool_calls: Option<serde_json::Value>,
}

impl ProviderMessage {
    pub fn as_json(&self) -> serde_json::Value {
        match self.role {
            Role::User => serde_json::json!({
                "role": "user",
                "content": [{"type": "input_text", "text": self.content}],
            }),
            Role::Assistant => {
                let mut content = Vec::new();
                if !self.content.is_empty() {
                    content.push(serde_json::json!({"type": "output_text", "text": self.content}));
                }
                if let Some(ref tc) = self.tool_calls {
                    // Faking tool_calls as output_text since 'tool_calls' type is rejected in content array.
                    let tc_str = format!("\n[tool_calls: {}]", serde_json::to_string(tc).unwrap_or_default());
                    content.push(serde_json::json!({"type": "output_text", "text": tc_str}));
                }
                serde_json::json!({
                    "role": "assistant",
                    "content": content,
                })
            }
            Role::System => serde_json::json!({
                "role": "system",
                "content": [{"type": "text", "text": self.content}],
            }),
            Role::ToolResult => {
                 // Faking tool results as user input_text since 'tool' role is likely rejected or needs faking.
                 let content_str = format!("[tool_results: {}]", self.content);
                 serde_json::json!({
                    "role": "user",
                    "content": [{"type": "input_text", "text": content_str}],
                })
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderRequest {
    pub messages: Vec<ProviderMessage>,
    pub model: String,
    pub board: Option<serde_json::Value>,
    pub custom_prompt: Option<String>,
}

#[derive(Debug)]
pub enum WorkerCmd {
    Send {
        session_id: String,
        request: ProviderRequest,
    },
    UpdateProvider {
        provider: Box<dyn Provider>,
    },
    #[allow(dead_code)]
    Shutdown,
}

#[derive(Debug)]
pub enum WorkerEvent {
    Delta { session_id: String, delta: String },
    Done { session_id: String },
    SystemNote { session_id: String, note: String },
    ToolStatus { session_id: String, status: String },
    ToolCalls { session_id: String, calls: serde_json::Value },
    ToolResult { session_id: String, content: String },
    BoardUpdate { board: serde_json::Value },
    Error { session_id: String, err: String },
}
