pub mod codex;

use std::sync::mpsc::Sender;

use anyhow::Result;
use serde_json::Value;

use crate::agent::WorkerEvent;
use crate::auth::CodexAuth;

pub use codex::CodexClient;

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

impl ToolCall {
    pub fn new(id: impl Into<String>, name: impl Into<String>, args: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            args,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModelTurnRequest {
    pub body: Value,
    pub emit_text_events: bool,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct ModelTurnResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    pub raw_payload: Option<Value>,
}

pub trait ModelTurnTransport: std::fmt::Debug + Send + Sync {
    /// Providers must normalize any backend-specific tool-call payloads into the
    /// canonical `ToolCall { id, name, args }` shape before returning.
    fn execute_turn(
        &self,
        auth: &CodexAuth,
        request: &ModelTurnRequest,
        session_id: String,
        turn_id: String,
        tx: &Sender<WorkerEvent>,
    ) -> Result<ModelTurnResponse>;
}
