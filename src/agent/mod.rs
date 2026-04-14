pub mod provider;
pub mod worker;

pub use provider::Provider;
pub use worker::WorkerHandles;

use crate::models::{
    AgentKind, AgentPhase, BackgroundJob, Board, ExecutionArtifact, Role, TurnStepStatus,
};
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
                serde_json::json!({
                    "role": "assistant",
                    "content": content,
                })
            }
            Role::System => serde_json::json!({
                "role": "system",
                "content": [{"type": "input_text", "text": self.content}],
            }),
            Role::ToolResult => {
                serde_json::json!({
                    "role": "system",
                    "content": [{"type": "input_text", "text": self.content}],
                })
            }
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
