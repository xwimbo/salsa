pub mod provider;
pub mod worker;

pub use provider::Provider;
pub use worker::WorkerHandles;

use crate::models::Role;

#[derive(Debug, Clone)]
pub struct ProviderMessage {
    pub role: Role,
    pub content: String,
}

impl ProviderMessage {
    pub fn as_json(&self) -> serde_json::Value {
        match self.role {
            Role::User => serde_json::json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": self.content}],
            }),
            Role::Assistant => serde_json::json!({
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": self.content}],
            }),
            Role::System => serde_json::json!({
                "type": "message",
                "role": "system",
                "content": [{"type": "text", "text": self.content}],
            }),
            Role::ToolResult => serde_json::json!({
                "type": "message",
                "role": "tool",
                "content": serde_json::from_str::<Vec<serde_json::Value>>(&self.content).unwrap_or_default(),
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderRequest {
    pub messages: Vec<ProviderMessage>,
    pub model: String,
    pub board: Option<serde_json::Value>,
}

#[derive(Debug)]
pub enum WorkerCmd {
    Send {
        session_id: u64,
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
    Delta { session_id: u64, delta: String },
    Done { session_id: u64 },
    SystemNote { session_id: u64, note: String },
    ToolStatus { session_id: u64, status: String },
    BoardUpdate { board: serde_json::Value },
    Error { session_id: u64, err: String },
}
