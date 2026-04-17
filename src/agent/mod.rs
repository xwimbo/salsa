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
    pub attachments: Vec<ProviderAttachment>,
}

impl ProviderMessage {
    pub fn as_json(&self) -> serde_json::Value {
        // Legacy Responses transport serializer. This intentionally emits
        // `input_text`/`output_text` content items instead of Chat Completions
        // `messages`, so migration work can isolate and replace this path later.
        let mut content = Vec::new();
        match self.role {
            Role::User | Role::System | Role::ToolResult => {
                if !self.content.is_empty() {
                    content.push(serde_json::json!({
                        "type": "input_text",
                        "text": self.content
                    }));
                }
                for attachment in &self.attachments {
                    content.push(attachment.as_json());
                }
            }
            Role::Assistant => {
                if !self.content.is_empty() {
                    content.push(serde_json::json!({
                        "type": "output_text",
                        "text": self.content
                    }));
                }
            }
        }

        match self.role {
            Role::User => serde_json::json!({ "role": "user", "content": content }),
            Role::Assistant => serde_json::json!({ "role": "assistant", "content": content }),
            Role::System | Role::ToolResult => serde_json::json!({ "role": "system", "content": content }),
        }
    }

    pub fn as_chat_completion_message(&self) -> serde_json::Value {
        let role = match self.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            // Until step 9 defines native tool-result feedback, keep tool results
            // as synthetic system context in the Chat Completions message stream.
            Role::ToolResult => "system",
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
    fn as_json(&self) -> serde_json::Value {
        match self {
            ProviderAttachment::Image {
                mime_type,
                data_base64,
            } => serde_json::json!({
                "type": "input_image",
                "image_url": format!("data:{};base64,{}", mime_type, data_base64),
            }),
            ProviderAttachment::File {
                mime_type,
                filename,
                data_base64,
            } => serde_json::json!({
                "type": "input_file",
                "filename": filename,
                "file_data": format!("data:{};base64,{}", mime_type, data_base64),
            }),
        }
    }

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
