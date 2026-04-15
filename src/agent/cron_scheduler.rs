use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use chrono::{Local, Timelike};
use uuid::Uuid;

use crate::agent::provider::run_specialist_loop;
use crate::agent::{ProviderMessage, ProviderRequest, WorkerEvent};
use crate::api::codex::CodexClient;
use crate::auth::CodexAuth;
use crate::models::{AgentKind, BackgroundJob, Board, JobStatus, Role};
use crate::tools::cron::pop_due_tasks;
use crate::tools::Sandbox;

/// Starts a background thread that checks for due cron tasks once per minute.
/// Due tasks are executed as sub-agent loops, with results flowing through the
/// existing WorkerEvent channel to the TUI.
pub fn start_cron_scheduler(
    auth: CodexAuth,
    client: CodexClient,
    sandbox: Sandbox,
    model: String,
    event_tx: Sender<WorkerEvent>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        loop {
            // Sleep until the next minute boundary
            let now = Local::now();
            let secs_into_minute = now.second();
            let sleep_secs = if secs_into_minute == 0 { 60 } else { 60 - secs_into_minute };
            thread::sleep(Duration::from_secs(sleep_secs as u64));

            let due_tasks = pop_due_tasks();
            if due_tasks.is_empty() {
                continue;
            }

            for task in due_tasks {
                let auth = auth.clone();
                let client = client.clone();
                let sandbox = sandbox.clone();
                let model = model.clone();
                let tx = event_tx.clone();

                thread::spawn(move || {
                    let job_id = Uuid::new_v4().to_string();
                    let session_id = format!("cron-{}", &job_id[..8]);

                    let _ = tx.send(WorkerEvent::JobStarted {
                        session_id: session_id.clone(),
                        job: BackgroundJob {
                            id: job_id.clone(),
                            agent: AgentKind::Coder,
                            title: format!("Cron: {}", if task.prompt.len() > 40 {
                                format!("{}…", &task.prompt[..37])
                            } else {
                                task.prompt.clone()
                            }),
                            status: JobStatus::Running,
                            project_id: None,
                            summary: "running scheduled task...".to_string(),
                        },
                    });

                    let request = ProviderRequest {
                        messages: vec![ProviderMessage {
                            role: Role::User,
                            content: task.prompt.clone(),
                            tool_calls: None,
                            attachments: Vec::new(),
                        }],
                        model,
                        project_id: None,
                        board: Some(Board::default()),
                        custom_prompt: None,
                        agent: AgentKind::Coder,
                    };

                    match run_specialist_loop(
                        &auth,
                        &client,
                        &sandbox,
                        &request,
                        AgentKind::Coder,
                        "cron-task",
                        None,
                        Some((&session_id, &job_id, &tx)),
                        true,
                    ) {
                        Ok(result) => {
                            let _ = tx.send(WorkerEvent::JobUpdated {
                                session_id: session_id.clone(),
                                job_id: job_id.clone(),
                                status: JobStatus::Completed,
                                summary: "completed".to_string(),
                            });
                            let _ = tx.send(WorkerEvent::JobMessage {
                                session_id,
                                job_id,
                                content: result,
                            });
                        }
                        Err(err) => {
                            let _ = tx.send(WorkerEvent::JobUpdated {
                                session_id: session_id.clone(),
                                job_id: job_id.clone(),
                                status: JobStatus::Failed,
                                summary: err.to_string(),
                            });
                            let _ = tx.send(WorkerEvent::JobMessage {
                                session_id,
                                job_id,
                                content: format!("[cron error] {}", err),
                            });
                        }
                    }
                });
            }
        }
    })
}
