use std::sync::mpsc::Sender;
use std::thread;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::provider::run_specialist_loop;
use crate::agent::{ProviderMessage, ProviderRequest, WorkerEvent};
use crate::api::CodexClient;
use crate::auth::CodexAuth;
use crate::models::{
    AgentKind, BackgroundJob, Board, BoardOperation, JobStatus, Role,
};
use crate::tools::Sandbox;

pub fn execute(
    auth: &CodexAuth,
    client: &CodexClient,
    sandbox: &Sandbox,
    args: &Value,
    session_id: &str,
    turn_id: &str,
    tx: &Sender<WorkerEvent>,
    model: &str,
    custom_prompt: Option<&str>,
) -> Result<(String, Vec<BoardOperation>)> {
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `description`"))?;
    let prompt = args
        .get("prompt")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `prompt`"))?;
    let run_in_background = args
        .get("run_in_background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let request = ProviderRequest {
        messages: vec![ProviderMessage {
            role: Role::User,
            content: prompt.to_string(),
            tool_calls: None,
            attachments: Vec::new(),
        }],
        model: model.to_string(),
        project_id: None,
        board: Some(Board::default()),
        custom_prompt: custom_prompt.map(|s| {
            format!(
                "{}\n\nSub-agent task: {}\nDescription: {}",
                s, prompt, description
            )
        }),
        agent: AgentKind::Coder,
    };

    if run_in_background {
        let job_id = Uuid::new_v4().to_string();
        let job = BackgroundJob {
            id: job_id.clone(),
            agent: AgentKind::Coder,
            title: description.to_string(),
            status: JobStatus::Queued,
            project_id: None,
            summary: "queued".to_string(),
        };

        let _ = tx.send(WorkerEvent::JobStarted {
            session_id: session_id.to_string(),
            job: job,
        });

        let auth = auth.clone();
        let client = client.clone();
        let sandbox = sandbox.clone();
        let session_id = session_id.to_string();
        let _turn_id = turn_id.to_string();
        let tx = tx.clone();
        let job_id_clone = job_id.clone();
        let request = request;

        thread::spawn(move || {
            let _ = tx.send(WorkerEvent::JobUpdated {
                session_id: session_id.clone(),
                job_id: job_id_clone.clone(),
                status: JobStatus::Running,
                summary: "running sub-agent...".to_string(),
            });

            match run_specialist_loop(
                &auth,
                &client,
                &sandbox,
                &request,
                AgentKind::Coder,
                "sub-agent",
                None,
                Some((&session_id, &job_id_clone, &tx)),
                true, // is_subagent
            ) {
                Ok(result) => {
                    let _ = tx.send(WorkerEvent::JobUpdated {
                        session_id: session_id.clone(),
                        job_id: job_id_clone.clone(),
                        status: JobStatus::Completed,
                        summary: "completed".to_string(),
                    });
                    let _ = tx.send(WorkerEvent::JobMessage {
                        session_id,
                        job_id: job_id_clone,
                        content: result,
                    });
                }
                Err(err) => {
                    let _ = tx.send(WorkerEvent::JobUpdated {
                        session_id: session_id.clone(),
                        job_id: job_id_clone.clone(),
                        status: JobStatus::Failed,
                        summary: err.to_string(),
                    });
                    let _ = tx.send(WorkerEvent::JobMessage {
                        session_id,
                        job_id: job_id_clone,
                        content: format!("[sub-agent error] {}", err),
                    });
                }
            }
        });

        Ok((
            format!(
                "Sub-agent launched in background (job {}). Description: {}",
                job_id, description
            ),
            Vec::new(),
        ))
    } else {
        // Foreground: run on current thread
        let result = run_specialist_loop(
            auth,
            client,
            sandbox,
            &request,
            AgentKind::Coder,
            "sub-agent",
            Some((session_id, turn_id, tx)),
            None,
            true, // is_subagent
        )?;

        Ok((result, Vec::new()))
    }
}

pub fn spec() -> Value {
    json!({
        "type": "function",
        "name": "agent",
        "description": "Spawn a sub-agent to handle a task. The sub-agent gets its own planning loop with workspace access. Can run in foreground (blocks until done) or background (returns immediately with a job ID).",
        "parameters": {
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Short description of what the sub-agent should do."
                },
                "prompt": {
                    "type": "string",
                    "description": "The full prompt/instructions for the sub-agent."
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "If true, the sub-agent runs in the background and returns a job ID. Default: false."
                }
            },
            "required": ["description", "prompt"],
            "additionalProperties": false
        }
    })
}
