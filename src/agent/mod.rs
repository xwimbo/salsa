pub mod cron_scheduler;
pub mod provider;
pub mod worker;

pub use provider::Provider;
pub use worker::WorkerHandles;

use crate::models::{
    AgentKind, AgentPhase, BackgroundJob, Board, ExecutionArtifact, Role, TurnStepStatus,
};
use serde_json;

#[derive(Debug, Clone)]
pub enum ProviderAttachment {
    Image {
        mime_type: String,
        data_base64: String,
    },
    File {
        mime_type: String,
        filename: String,
        data_base64: String,
    },
}

#[derive(Debug, Clone)]
pub struct ProviderMessage {
    pub role: Role,
    pub content: String,
    pub tool_calls: Option<serde_json::Value>,
    pub tool_call_id: Option<String>,
    pub attachments: Vec<ProviderAttachment>,
}

impl ProviderMessage {
    // `ProviderMessage` remains the app-facing conversation type; transport-
    // specific shaping is isolated to this serializer so the rest of the app can
    // keep working with a stable message model across normal chat, tool calls,
    // tool results, system instructions, and attachment-bearing context.
    pub fn as_chat_completion_message(&self) -> serde_json::Value {
        if matches!(self.role, Role::Assistant) && self.tool_calls.is_some() {
            return serde_json::json!({
                "role": "assistant",
                "content": if self.content.is_empty() { serde_json::Value::Null } else { serde_json::json!(self.content) },
                "tool_calls": self.tool_calls.clone().unwrap_or_else(|| serde_json::json!([])),
            });
        }

        if matches!(self.role, Role::ToolResult) {
            return serde_json::json!({
                "role": "tool",
                "tool_call_id": self.tool_call_id.clone().unwrap_or_default(),
                "content": self.content,
            });
        }

        let role = match self.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            Role::ToolResult => "tool",
        };

        let mut content = Vec::new();
        if !self.content.is_empty() {
            content.push(serde_json::json!({
                "type": "text",
                "text": self.content,
            }));
        }
        for attachment in &self.attachments {
            if let Some(item) = attachment.as_chat_completion_content_part() {
                content.push(item);
            }
        }

        if content.is_empty() {
            serde_json::json!({
                "role": role,
                "content": self.content,
            })
        } else if content.len() == 1 && content[0].get("type").and_then(|v| v.as_str()) == Some("text") {
            serde_json::json!({
                "role": role,
                "content": content[0].get("text").cloned().unwrap_or_else(|| serde_json::json!("")),
            })
        } else {
            serde_json::json!({
                "role": role,
                "content": content,
            })
        }
    }
}

impl ProviderAttachment {
    fn as_chat_completion_content_part(&self) -> Option<serde_json::Value> {
        match self {
            ProviderAttachment::Image {
                mime_type,
                data_base64,
            } => Some(serde_json::json!({
                "type": "image_url",
                "image_url": {
                    "url": format!("data:{};base64,{}", mime_type, data_base64),
                }
            })),
            ProviderAttachment::File {
                mime_type,
                filename,
                data_base64,
            } => Some(serde_json::json!({
                "type": "file",
                "file": {
                    "filename": filename,
                    "file_data": format!("data:{};base64,{}", mime_type, data_base64),
                }
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ProviderAttachment, ProviderMessage};
    use crate::models::Role;

    #[test]
    fn serializes_plain_text_messages_for_chat_completions() {
        let system = ProviderMessage {
            role: Role::System,
            content: "follow the plan".into(),
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
        };
        let user = ProviderMessage {
            role: Role::User,
            content: "hello".into(),
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
        };

        assert_eq!(
            system.as_chat_completion_message(),
            serde_json::json!({"role": "system", "content": "follow the plan"})
        );
        assert_eq!(
            user.as_chat_completion_message(),
            serde_json::json!({"role": "user", "content": "hello"})
        );
    }

    #[test]
    fn serializes_assistant_tool_calls_for_chat_completions() {
        let message = ProviderMessage {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: Some(serde_json::json!([
                {
                    "id": "call-1",
                    "type": "function",
                    "function": {"name": "fs_list", "arguments": "{\"path\":\".\"}"}
                }
            ])),
            tool_call_id: None,
            attachments: Vec::new(),
        };

        assert_eq!(
            message.as_chat_completion_message(),
            serde_json::json!({
                "role": "assistant",
                "content": serde_json::Value::Null,
                "tool_calls": [
                    {
                        "id": "call-1",
                        "type": "function",
                        "function": {"name": "fs_list", "arguments": "{\"path\":\".\"}"}
                    }
                ]
            })
        );
    }

    #[test]
    fn serializes_tool_result_messages_for_chat_completions() {
        let message = ProviderMessage {
            role: Role::ToolResult,
            content: "workspace entries: src".into(),
            tool_calls: None,
            tool_call_id: Some("call-1".into()),
            attachments: Vec::new(),
        };

        assert_eq!(
            message.as_chat_completion_message(),
            serde_json::json!({
                "role": "tool",
                "tool_call_id": "call-1",
                "content": "workspace entries: src"
            })
        );
    }

    #[test]
    fn serializes_attachment_messages_as_content_parts() {
        let message = ProviderMessage {
            role: Role::User,
            content: "inspect this image".into(),
            tool_calls: None,
            tool_call_id: None,
            attachments: vec![ProviderAttachment::Image {
                mime_type: "image/png".into(),
                data_base64: "abc123".into(),
            }],
        };

        assert_eq!(
            message.as_chat_completion_message(),
            serde_json::json!({
                "role": "user",
                "content": [
                    {"type": "text", "text": "inspect this image"},
                    {
                        "type": "image_url",
                        "image_url": {"url": "data:image/png;base64,abc123"}
                    }
                ]
            })
        );
    }
}

#[derive(Debug, Clone)]
pub struct ProviderRequest {
    pub messages: Vec<ProviderMessage>,
    pub model: String,
    pub project_id: Option<String>,
    pub board: Option<Board>,
    pub custom_prompt: Option<String>,
    pub agent: AgentKind,
}

#[derive(Debug)]
pub enum WorkerCmd {
    Send {
        turn_id: String,
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
    Delta {
        session_id: String,
        turn_id: String,
        delta: String,
    },
    Done {
        session_id: String,
        turn_id: String,
    },
    SystemNote {
        session_id: String,
        turn_id: String,
        note: String,
    },
    ToolStatus {
        session_id: String,
        turn_id: String,
        status: String,
    },
    ToolCalls {
        session_id: String,
        turn_id: String,
        calls: serde_json::Value,
    },
    PhaseChange {
        session_id: String,
        turn_id: String,
        phase: AgentPhase,
    },
    StepUpdate {
        session_id: String,
        turn_id: String,
        phase: AgentPhase,
        status: TurnStepStatus,
        summary: Option<String>,
    },
    StepArtifact {
        session_id: String,
        turn_id: String,
        phase: AgentPhase,
        artifact: ExecutionArtifact,
    },
    BoardUpdate {
        session_id: String,
        turn_id: String,
        project_id: Option<String>,
        operations: Vec<crate::models::BoardOperation>,
    },
    JobStarted {
        session_id: String,
        job: BackgroundJob,
    },
    JobUpdated {
        session_id: String,
        job_id: String,
        status: crate::models::JobStatus,
        summary: String,
    },
    JobMessage {
        session_id: String,
        job_id: String,
        content: String,
    },
    Error {
        session_id: String,
        turn_id: String,
        err: String,
    },
}
