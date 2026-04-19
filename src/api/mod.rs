pub mod codex;
pub mod local;

use std::sync::mpsc::Sender;

use anyhow::Result;
use serde_json::Value;

use crate::agent::WorkerEvent;
use crate::auth::CodexAuth;

pub use codex::CodexClient;
pub use local::LocalChatCompletionsClient;

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
pub struct ChatCompletionsStreamState {
    text: String,
    tool_calls: Vec<ChatCompletionsToolCallState>,
    finish_reason: Option<String>,
    last_event: Option<Value>,
}

impl ChatCompletionsStreamState {
    pub fn ingest_event(
        &mut self,
        data: &str,
        session_id: &str,
        turn_id: &str,
        emit_text_events: bool,
        tx: &Sender<WorkerEvent>,
    ) -> Result<bool> {
        if data.trim() == "[DONE]" {
            return Ok(true);
        }

        let value: Value = serde_json::from_str(data)?;
        self.last_event = Some(value.clone());

        if let Some(error_message) = chat_completions_error_message(&value) {
            return Err(anyhow::anyhow!(error_message));
        }

        self.capture_finish_reason(&value);
        self.ingest_text_delta(&value, session_id, turn_id, emit_text_events, tx);
        self.ingest_tool_call_delta(&value);

        Ok(false)
    }

    pub fn finalize(self) -> ModelTurnResponse {
        ModelTurnResponse {
            text: self.text,
            tool_calls: self
                .tool_calls
                .into_iter()
                .filter_map(ChatCompletionsToolCallState::finalize)
                .collect(),
            finish_reason: self.finish_reason,
            raw_payload: self.last_event,
        }
    }

    fn capture_finish_reason(&mut self, value: &Value) {
        if self.finish_reason.is_none() {
            self.finish_reason = value
                .pointer("/choices/0/finish_reason")
                .and_then(|reason| reason.as_str())
                .map(ToOwned::to_owned)
                .or_else(|| {
                    value.get("finish_reason")
                        .and_then(|reason| reason.as_str())
                        .map(ToOwned::to_owned)
                });
        }
    }

    fn ingest_text_delta(
        &mut self,
        value: &Value,
        session_id: &str,
        turn_id: &str,
        emit_text_events: bool,
        tx: &Sender<WorkerEvent>,
    ) {
        for delta in extract_chat_completion_text_deltas(value) {
            self.text.push_str(&delta);
            if emit_text_events {
                let _ = tx.send(WorkerEvent::Delta {
                    session_id: session_id.to_string(),
                    turn_id: turn_id.to_string(),
                    delta,
                });
            }
        }
    }

    fn ingest_tool_call_delta(&mut self, value: &Value) {
        if let Some(tool_calls) = value
            .pointer("/choices/0/delta/tool_calls")
            .and_then(|tool_calls| tool_calls.as_array())
        {
            for tool_call in tool_calls {
                self.merge_tool_call_delta(tool_call);
            }
        }

        if let Some(tool_calls) = value.pointer("/choices/0/message/tool_calls") {
            self.ingest_tool_call_snapshot(tool_calls);
        }
        if let Some(tool_calls) = value.pointer("/message/tool_calls") {
            self.ingest_tool_call_snapshot(tool_calls);
        }
    }

    fn ingest_tool_call_snapshot(&mut self, tool_calls: &Value) {
        if let Some(tool_calls) = tool_calls.as_array() {
            for tool_call in tool_calls {
                self.merge_tool_call_snapshot(tool_call);
            }
        }
    }

    fn merge_tool_call_delta(&mut self, tool_call: &Value) {
        let index = tool_call
            .get("index")
            .and_then(|index| index.as_u64())
            .map(|index| index as usize)
            .unwrap_or_else(|| self.tool_calls.len());
        while self.tool_calls.len() <= index {
            self.tool_calls.push(ChatCompletionsToolCallState::default());
        }
        self.tool_calls[index].merge_delta(tool_call);
    }

    fn merge_tool_call_snapshot(&mut self, tool_call: &Value) {
        let index = tool_call
            .get("index")
            .and_then(|index| index.as_u64())
            .map(|index| index as usize)
            .unwrap_or_else(|| self.tool_calls.len());
        while self.tool_calls.len() <= index {
            self.tool_calls.push(ChatCompletionsToolCallState::default());
        }
        self.tool_calls[index].merge_snapshot(tool_call);
    }
}

#[derive(Debug, Clone, Default)]
struct ChatCompletionsToolCallState {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ChatCompletionsToolCallState {
    fn merge_delta(&mut self, tool_call: &Value) {
        if let Some(id) = tool_call.get("id").and_then(|id| id.as_str()) {
            self.id = Some(id.to_string());
        }

        let function = tool_call.get("function").unwrap_or(tool_call);
        if let Some(name) = function.get("name").and_then(|name| name.as_str()) {
            self.name = Some(name.to_string());
        }
        if let Some(arguments) = function.get("arguments").and_then(|arguments| arguments.as_str()) {
            self.arguments.push_str(arguments);
        }
    }

    fn merge_snapshot(&mut self, tool_call: &Value) {
        self.merge_delta(tool_call);
    }

    fn finalize(self) -> Option<ToolCall> {
        let name = self.name?;
        let args = if self.arguments.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&self.arguments).unwrap_or(Value::String(self.arguments.clone()))
        };
        Some(ToolCall::new(self.id.unwrap_or_default(), name, args))
    }
}

fn extract_chat_completion_text_deltas(value: &Value) -> Vec<String> {
    let mut deltas = Vec::new();

    if let Some(content) = value.pointer("/choices/0/delta/content") {
        match content {
            Value::String(text) => deltas.push(text.clone()),
            Value::Array(parts) => {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(|text| text.as_str()) {
                        deltas.push(text.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    if deltas.is_empty() {
        if let Some(content) = value.pointer("/choices/0/message/content") {
            match content {
                Value::String(text) => deltas.push(text.clone()),
                Value::Array(parts) => {
                    for part in parts {
                        if part.get("type").and_then(|part_type| part_type.as_str()) == Some("text") {
                            if let Some(text) = part.get("text").and_then(|text| text.as_str()) {
                                deltas.push(text.to_string());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    deltas
}

fn chat_completions_error_message(value: &Value) -> Option<String> {
    value
        .pointer("/error/message")
        .or_else(|| value.get("message"))
        .and_then(|message| message.as_str())
        .map(ToOwned::to_owned)
}

#[derive(Debug, Clone)]
pub struct ChatCompletionsRequestBuilder {
    model: String,
    messages: Vec<Value>,
    tools: Vec<Value>,
    tool_choice: Option<Value>,
    emit_text_events: bool,
}

impl ChatCompletionsRequestBuilder {
    pub fn direct(model: impl Into<String>, messages: Vec<Value>) -> Self {
        Self {
            model: model.into(),
            messages,
            tools: Vec::new(),
            tool_choice: Some(serde_json::json!("none")),
            emit_text_events: true,
        }
    }

    pub fn specialist(model: impl Into<String>, messages: Vec<Value>, tools: Vec<Value>) -> Self {
        let tool_choice = if tools.is_empty() {
            serde_json::json!("none")
        } else {
            serde_json::json!("auto")
        };
        Self {
            model: model.into(),
            messages,
            tools,
            tool_choice: Some(tool_choice),
            emit_text_events: false,
        }
    }

    pub fn tool_choice(mut self, tool_choice: impl Into<Value>) -> Self {
        self.tool_choice = Some(tool_choice.into());
        self
    }

    pub fn emit_text_events(mut self, emit_text_events: bool) -> Self {
        self.emit_text_events = emit_text_events;
        self
    }

    pub fn build(self) -> ModelTurnRequest {
        ModelTurnRequest {
            body: serde_json::json!({
                "model": self.model,
                "messages": self.messages,
                "tools": self.tools,
                "tool_choice": self.tool_choice.unwrap_or_else(|| serde_json::json!("none")),
                "parallel_tool_calls": false,
                "stream": true,
            }),
            emit_text_events: self.emit_text_events,
        }
    }
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::{ChatCompletionsRequestBuilder, ChatCompletionsStreamState};
    use crate::agent::WorkerEvent;

    #[test]
    fn request_builder_emits_chat_completions_shape() {
        let request = ChatCompletionsRequestBuilder::specialist(
            "stub-model",
            vec![serde_json::json!({"role": "system", "content": "plan"})],
            vec![serde_json::json!({
                "type": "function",
                "function": {
                    "name": "fs_read",
                    "description": "read",
                    "parameters": {"type": "object"}
                }
            })],
        )
        .tool_choice(serde_json::json!("auto"))
        .emit_text_events(false)
        .build();

        assert_eq!(request.body["model"], serde_json::json!("stub-model"));
        assert_eq!(request.body["messages"][0]["role"], serde_json::json!("system"));
        assert_eq!(request.body["tools"][0]["function"]["name"], serde_json::json!("fs_read"));
        assert_eq!(request.body["tool_choice"], serde_json::json!("auto"));
        assert_eq!(request.body["stream"], serde_json::json!(true));
        assert!(!request.emit_text_events);
    }

    #[test]
    fn stream_state_accumulates_plain_text_deltas() {
        let (tx, rx) = mpsc::channel();
        let mut state = ChatCompletionsStreamState::default();

        let done = state
            .ingest_event(
                r#"{"choices":[{"delta":{"content":"hello "}}]}"#,
                "session-1",
                "turn-1",
                true,
                &tx,
            )
            .unwrap();
        assert!(!done);
        state
            .ingest_event(
                r#"{"choices":[{"delta":{"content":"world"},"finish_reason":"stop"}]}"#,
                "session-1",
                "turn-1",
                true,
                &tx,
            )
            .unwrap();

        let response = state.finalize();
        assert_eq!(response.text, "hello world");
        assert_eq!(response.finish_reason.as_deref(), Some("stop"));

        let deltas: Vec<String> = rx
            .try_iter()
            .filter_map(|event| match event {
                WorkerEvent::Delta { delta, .. } => Some(delta),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, vec!["hello ".to_string(), "world".to_string()]);
    }

    #[test]
    fn stream_state_reconstructs_fragmented_tool_calls() {
        let (tx, _rx) = mpsc::channel();
        let mut state = ChatCompletionsStreamState::default();

        state
            .ingest_event(
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"fs_read","arguments":"{\"path\""}}]}}]}"#,
                "session-1",
                "turn-1",
                false,
                &tx,
            )
            .unwrap();
        state
            .ingest_event(
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"src/main.rs\"}"}}]},"finish_reason":"tool_calls"}]}"#,
                "session-1",
                "turn-1",
                false,
                &tx,
            )
            .unwrap();

        let response = state.finalize();
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "call-1");
        assert_eq!(response.tool_calls[0].name, "fs_read");
        assert_eq!(response.tool_calls[0].args, serde_json::json!({"path": "src/main.rs"}));
        assert_eq!(response.finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn stream_state_surfaces_backend_errors() {
        let (tx, _rx) = mpsc::channel();
        let mut state = ChatCompletionsStreamState::default();

        let err = state
            .ingest_event(
                r#"{"error":{"message":"bad request"}}"#,
                "session-1",
                "turn-1",
                false,
                &tx,
            )
            .unwrap_err();

        assert!(err.to_string().contains("bad request"));
    }
}
