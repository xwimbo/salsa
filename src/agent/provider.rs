use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use uuid::Uuid;

use crate::agent::{ProviderMessage, ProviderRequest, WorkerEvent};
use crate::api::{
    ChatCompletionsRequestBuilder, CodexClient, LocalChatCompletionsClient, ModelTurnRequest,
    ModelTurnTransport, ToolCall,
};
use crate::auth::CodexAuth;
use crate::models::{
    AgentKind, AgentPhase, BackgroundJob, Board, BoardOperation, ContinuationFrame,
    ExecutionArtifact, JobStatus, Role, TurnStepStatus,
};
use crate::tools::{self, Sandbox};

pub trait Provider: std::fmt::Debug + Send + Sync + 'static {
    fn generate(
        &self,
        request: &ProviderRequest,
        session_id: String,
        turn_id: String,
        tx: &Sender<WorkerEvent>,
    );
    fn label(&self) -> &'static str;
}

#[derive(Debug)]
pub struct EchoProvider;

impl Provider for EchoProvider {
    fn label(&self) -> &'static str {
        "echo"
    }

    fn generate(
        &self,
        request: &ProviderRequest,
        session_id: String,
        turn_id: String,
        tx: &Sender<WorkerEvent>,
    ) {
        let last_user = request
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, Role::User))
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let reply = if last_user.trim().is_empty() {
            "(echo provider) empty input.".to_string()
        } else {
            format!(
                "You said: \"{}\". That's {} characters. The phased planner loop is now wired into the real provider.",
                last_user.trim(),
                last_user.chars().count()
            )
        };

        for chunk in reply.split_inclusive(' ') {
            thread::sleep(Duration::from_millis(35));
            if tx
                .send(WorkerEvent::Delta {
                    session_id: session_id.clone(),
                    turn_id: turn_id.clone(),
                    delta: chunk.to_string(),
                })
                .is_err()
            {
                return;
            }
        }

        let _ = tx.send(WorkerEvent::Done {
            session_id,
            turn_id,
        });
    }
}

#[derive(Debug, Clone)]
pub struct StubCompletionsProvider {
    sandbox: Sandbox,
}

impl StubCompletionsProvider {
    pub fn new(workspace: PathBuf) -> Result<Self> {
        Ok(Self {
            sandbox: Sandbox::new(workspace)?,
        })
    }
}

impl Provider for StubCompletionsProvider {
    fn label(&self) -> &'static str {
        "stub"
    }

    fn generate(
        &self,
        request: &ProviderRequest,
        session_id: String,
        turn_id: String,
        tx: &Sender<WorkerEvent>,
    ) {
        let result = match request.agent {
            AgentKind::Coder => run_stub_specialist_turn(
                &self.sandbox,
                request,
                AgentKind::Coder,
                "coder",
                session_id.clone(),
                turn_id.clone(),
                tx,
            ),
            AgentKind::Analyst => run_stub_specialist_turn(
                &self.sandbox,
                request,
                AgentKind::Analyst,
                "analyst",
                session_id.clone(),
                turn_id.clone(),
                tx,
            ),
            AgentKind::Planner => {
                stream_stub_text_response(request, session_id.clone(), turn_id.clone(), tx, true)
            }
            AgentKind::Orchestrator => match route_request(request) {
                RouteDecision::Direct | RouteDecision::Planner => {
                    stream_stub_text_response(request, session_id.clone(), turn_id.clone(), tx, true)
                }
                RouteDecision::Analyst => run_stub_specialist_turn(
                    &self.sandbox,
                    request,
                    AgentKind::Analyst,
                    "analyst",
                    session_id.clone(),
                    turn_id.clone(),
                    tx,
                ),
                RouteDecision::Coder => run_stub_specialist_turn(
                    &self.sandbox,
                    request,
                    AgentKind::Coder,
                    "coder",
                    session_id.clone(),
                    turn_id.clone(),
                    tx,
                ),
            },
        };

        match result {
            Ok(()) => {
                let _ = tx.send(WorkerEvent::Done {
                    session_id,
                    turn_id,
                });
            }
            Err(err) => {
                let _ = tx.send(WorkerEvent::Error {
                    session_id,
                    turn_id,
                    err: err.to_string(),
                });
            }
        }
    }
}

#[derive(Debug)]
pub struct CodexProvider {
    auth: CodexAuth,
    transport: CodexClient,
    sandbox: Sandbox,
}

#[derive(Debug)]
pub struct LocalChatCompletionsProvider {
    auth: CodexAuth,
    transport: LocalChatCompletionsClient,
}

impl CodexProvider {
    pub fn new(auth: CodexAuth, workspace: PathBuf) -> Result<Self> {
        let transport = CodexClient::new()?;
        let sandbox = Sandbox::new(workspace)?;
        Ok(Self {
            auth,
            transport,
            sandbox,
        })
    }
}

impl LocalChatCompletionsProvider {
    pub fn new(
        auth: CodexAuth,
        workspace: PathBuf,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self> {
        let transport = LocalChatCompletionsClient::new(base_url, api_key, timeout)?;
        let _ = Sandbox::new(workspace)?;
        Ok(Self { auth, transport })
    }
}

impl Provider for CodexProvider {
    fn label(&self) -> &'static str {
        "orchestrator"
    }

    fn generate(
        &self,
        request: &ProviderRequest,
        session_id: String,
        turn_id: String,
        tx: &Sender<WorkerEvent>,
    ) {
        generate_codex_provider(
            &self.auth,
            &self.transport,
            &self.sandbox,
            request,
            session_id,
            turn_id,
            tx,
        );
    }
}

impl Provider for LocalChatCompletionsProvider {
    fn label(&self) -> &'static str {
        "local"
    }

    fn generate(
        &self,
        request: &ProviderRequest,
        session_id: String,
        turn_id: String,
        tx: &Sender<WorkerEvent>,
    ) {
        let result = match request.agent {
            AgentKind::Coder | AgentKind::Analyst => stream_text_response(
                &self.auth,
                &self.transport,
                request,
                session_id.clone(),
                turn_id.clone(),
                tx,
                orchestrator_instructions(request),
            ),
            AgentKind::Planner => stream_text_response(
                &self.auth,
                &self.transport,
                request,
                session_id.clone(),
                turn_id.clone(),
                tx,
                planner_instructions(request),
            ),
            AgentKind::Orchestrator => stream_text_response(
                &self.auth,
                &self.transport,
                request,
                session_id.clone(),
                turn_id.clone(),
                tx,
                orchestrator_instructions(request),
            ),
        };

        if let Err(err) = result {
            let _ = tx.send(WorkerEvent::Error {
                session_id,
                turn_id,
                err: err.to_string(),
            });
        }
    }
}

fn generate_codex_provider(
    auth: &CodexAuth,
    transport: &CodexClient,
    sandbox: &Sandbox,
    request: &ProviderRequest,
    session_id: String,
    turn_id: String,
    tx: &Sender<WorkerEvent>,
) {
    match request.agent {
        AgentKind::Coder => {
            if let Err(err) = run_specialist_turn(
                auth,
                transport,
                sandbox,
                request,
                AgentKind::Coder,
                "coder",
                session_id.clone(),
                turn_id.clone(),
                tx,
            ) {
                let _ = tx.send(WorkerEvent::Error {
                    session_id,
                    turn_id,
                    err: err.to_string(),
                });
            }
        }
        AgentKind::Analyst => {
            if let Err(err) = run_specialist_turn(
                auth,
                transport,
                sandbox,
                request,
                AgentKind::Analyst,
                "analyst",
                session_id.clone(),
                turn_id.clone(),
                tx,
            ) {
                let _ = tx.send(WorkerEvent::Error {
                    session_id,
                    turn_id,
                    err: err.to_string(),
                });
            }
        }
        AgentKind::Planner => {
            if let Err(err) = stream_text_response(
                auth,
                transport,
                request,
                session_id.clone(),
                turn_id.clone(),
                tx,
                planner_instructions(request),
            ) {
                let _ = tx.send(WorkerEvent::Error {
                    session_id,
                    turn_id,
                    err: err.to_string(),
                });
            }
        }
        AgentKind::Orchestrator => match route_request(request) {
            RouteDecision::Direct => {
                if let Err(err) = stream_text_response(
                    auth,
                    transport,
                    request,
                    session_id.clone(),
                    turn_id.clone(),
                    tx,
                    orchestrator_instructions(request),
                ) {
                    let _ = tx.send(WorkerEvent::Error {
                        session_id,
                        turn_id,
                        err: err.to_string(),
                    });
                }
            }
            RouteDecision::Planner => {
                if let Err(err) = stream_text_response(
                    auth,
                    transport,
                    request,
                    session_id.clone(),
                    turn_id.clone(),
                    tx,
                    planner_instructions(request),
                ) {
                    let _ = tx.send(WorkerEvent::Error {
                        session_id,
                        turn_id,
                        err: err.to_string(),
                    });
                }
            }
            RouteDecision::Analyst => {
                spawn_background_specialist(
                    auth,
                    transport,
                    sandbox,
                    request,
                    AgentKind::Analyst,
                    "data analysis",
                    session_id,
                    turn_id,
                    tx,
                );
            }
            RouteDecision::Coder => {
                spawn_background_specialist(
                    auth,
                    transport,
                    sandbox,
                    request,
                    AgentKind::Coder,
                    "coding",
                    session_id,
                    turn_id,
                    tx,
                );
            }
        },
    }
}

fn spawn_background_specialist(
    auth: &CodexAuth,
    transport: &CodexClient,
    sandbox: &Sandbox,
    request: &ProviderRequest,
    agent: AgentKind,
    initial_summary: &str,
    session_id: String,
    turn_id: String,
    tx: &Sender<WorkerEvent>,
) {
    let job_id = Uuid::new_v4().to_string();
    let job = BackgroundJob {
        id: job_id.clone(),
        agent,
        title: summarize_job_title(request),
        status: JobStatus::Queued,
        project_id: request.project_id.clone(),
        summary: "queued".to_string(),
    };
    let _ = tx.send(WorkerEvent::JobStarted {
        session_id: session_id.clone(),
        job: job.clone(),
    });

    let initial_summary = initial_summary.to_string();

    let _ = tx.send(WorkerEvent::Delta {
        session_id: session_id.clone(),
        turn_id: turn_id.clone(),
        delta: format!(
            "I started a {} worker in the background. I’ll keep the conversation here and report back when it finishes.",
            initial_summary
        ),
    });

    let auth = auth.clone();
    let transport = transport.clone();
    let sandbox = sandbox.clone();
    let mut specialist_request = request.clone();
    specialist_request.agent = agent;
    let session_id_for_job = session_id.clone();
    let tx_for_job = tx.clone();
    thread::spawn(move || {
        let _ = tx_for_job.send(WorkerEvent::JobUpdated {
            session_id: session_id_for_job.clone(),
            job_id: job_id.clone(),
            status: JobStatus::Running,
            summary: format!("{}...", initial_summary),
        });

        match run_background_specialist_job(
            &auth,
            &transport,
            &sandbox,
            &specialist_request,
            session_id_for_job.clone(),
            job_id.clone(),
            &tx_for_job,
        ) {
            Ok(message) => {
                let _ = tx_for_job.send(WorkerEvent::JobUpdated {
                    session_id: session_id_for_job.clone(),
                    job_id: job_id.clone(),
                    status: JobStatus::Completed,
                    summary: "completed".to_string(),
                });
                let _ = tx_for_job.send(WorkerEvent::JobMessage {
                    session_id: session_id_for_job,
                    job_id,
                    content: message,
                });
            }
            Err(err) => {
                let _ = tx_for_job.send(WorkerEvent::JobUpdated {
                    session_id: session_id_for_job.clone(),
                    job_id: job_id.clone(),
                    status: JobStatus::Failed,
                    summary: err.to_string(),
                });
                let _ = tx_for_job.send(WorkerEvent::JobMessage {
                    session_id: session_id_for_job,
                    job_id,
                    content: format!("[background {} error] {}", initial_summary, err),
                });
            }
        }
    });

    let _ = tx.send(WorkerEvent::Done {
        session_id,
        turn_id,
    });
}

fn stream_text_response(
    auth: &CodexAuth,
    transport: &dyn ModelTurnTransport,
    request: &ProviderRequest,
    session_id: String,
    turn_id: String,
    tx: &Sender<WorkerEvent>,
    instructions: String,
) -> Result<()> {
    let turn_request = build_direct_turn_request(request, instructions);
    transport.execute_turn(auth, &turn_request, session_id.clone(), turn_id.clone(), tx)?;
    let _ = tx.send(WorkerEvent::Done {
        session_id,
        turn_id,
    });
    Ok(())
}

fn run_specialist_turn(
    auth: &CodexAuth,
    client: &CodexClient,
    sandbox: &Sandbox,
    request: &ProviderRequest,
    agent: AgentKind,
    label: &str,
    session_id: String,
    turn_id: String,
    tx: &Sender<WorkerEvent>,
) -> Result<()> {
    let summary = run_specialist_loop(
        auth,
        client,
        sandbox,
        request,
        agent,
        label,
        Some((&session_id, &turn_id, tx)),
        None,
        false,
    )?;
    let _ = summary;
    let _ = tx.send(WorkerEvent::Done {
        session_id,
        turn_id,
    });
    Ok(())
}

fn run_background_specialist_job(
    auth: &CodexAuth,
    client: &CodexClient,
    sandbox: &Sandbox,
    request: &ProviderRequest,
    session_id: String,
    job_id: String,
    tx: &Sender<WorkerEvent>,
) -> Result<String> {
    run_specialist_loop(
        auth,
        client,
        sandbox,
        request,
        request.agent,
        match request.agent {
            AgentKind::Coder => "coder",
            AgentKind::Analyst => "analyst",
            AgentKind::Orchestrator => "orchestrator",
            AgentKind::Planner => "planner",
        },
        None,
        Some((&session_id, &job_id, tx)),
        false,
    )
}

fn run_stub_specialist_turn(
    sandbox: &Sandbox,
    request: &ProviderRequest,
    agent: AgentKind,
    label: &str,
    session_id: String,
    turn_id: String,
    tx: &Sender<WorkerEvent>,
) -> Result<()> {
    let summary = run_stub_specialist_loop(
        sandbox,
        request,
        agent,
        label,
        Some((&session_id, &turn_id, tx)),
    )?;
    let _ = summary;
    Ok(())
}

fn stream_stub_text_response(
    request: &ProviderRequest,
    session_id: String,
    turn_id: String,
    tx: &Sender<WorkerEvent>,
    include_done: bool,
) -> Result<()> {
    let reply = stub_plain_response(request);
    stream_stub_delta_chunks(&reply, &session_id, &turn_id, tx)?;
    if include_done {
        let _ = tx.send(WorkerEvent::Done {
            session_id,
            turn_id,
        });
    }
    Ok(())
}

fn run_stub_specialist_loop(
    sandbox: &Sandbox,
    request: &ProviderRequest,
    agent: AgentKind,
    label: &str,
    interactive_turn: Option<(&str, &str, &Sender<WorkerEvent>)>,
) -> Result<String> {
    let mut conversation = request.messages.clone();
    let mut board = request
        .board
        .clone()
        .unwrap_or_default()
        .normalized_for_prompt();
    let last_user = latest_user_message(&conversation);
    let plan = StubToolPlan::from_prompt(&last_user, agent);
    let mut continuation_frames: Vec<ContinuationFrame> = Vec::new();
    let mut final_response = String::new();

    for phase in AgentPhase::ALL {
        if let Some((session_id, turn_id, tx)) = interactive_turn {
            let _ = tx.send(WorkerEvent::PhaseChange {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                phase,
            });
            let _ = tx.send(WorkerEvent::StepUpdate {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                phase,
                status: TurnStepStatus::Running,
                summary: Some(format!("stub {}", phase.status())),
            });
            let _ = tx.send(WorkerEvent::ToolStatus {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                status: format!("stub {}", phase.status()),
            });
        }

        board.apply_operations(&[BoardOperation::SetLastPhase { phase }]);
        let mut step_summary = format!("stub {} complete", phase.as_str());

        match phase {
            AgentPhase::Plan => {
                let plan_note = stub_phase_note(label, phase, &last_user, &plan);
                continuation_frames.push(build_continuation_frame(
                    phase,
                    &plan_note,
                    vec![ExecutionArtifact::AssistantNote {
                        text: plan_note.clone(),
                    }],
                ));
                compact_continuation_frames(&mut continuation_frames);
                step_summary = plan_note.clone();
                if let Some((session_id, turn_id, tx)) = interactive_turn {
                    let _ = tx.send(WorkerEvent::StepArtifact {
                        session_id: session_id.to_string(),
                        turn_id: turn_id.to_string(),
                        phase,
                        artifact: ExecutionArtifact::AssistantNote { text: plan_note },
                    });
                }
            }
            AgentPhase::Explore | AgentPhase::Act | AgentPhase::Verify => {
                if let Some(stub_call) = plan.call_for_phase(phase) {
                    let tool_call = stub_call.to_tool_call(phase);
                    let assistant_tool_calls = serde_json::json!([tool_call.as_chat_completion_call()]);
                    conversation.push(ProviderMessage {
                        role: Role::Assistant,
                        content: stub_call.assistant_preface(phase),
                        tool_calls: Some(assistant_tool_calls.clone()),
                        tool_call_id: None,
                        attachments: Vec::new(),
                    });

                    if let Some((session_id, turn_id, tx)) = interactive_turn {
                        let _ = tx.send(WorkerEvent::ToolCalls {
                            session_id: session_id.to_string(),
                            turn_id: turn_id.to_string(),
                            calls: assistant_tool_calls,
                        });
                        let _ = tx.send(WorkerEvent::SystemNote {
                            session_id: session_id.to_string(),
                            turn_id: turn_id.to_string(),
                            note: tools::tool_slug(&tool_call.name, &tool_call.args),
                        });
                        let _ = tx.send(WorkerEvent::StepArtifact {
                            session_id: session_id.to_string(),
                            turn_id: turn_id.to_string(),
                            phase,
                            artifact: ExecutionArtifact::ToolCall {
                                tool_name: tool_call.name.clone(),
                                args: tool_call.args.clone(),
                            },
                        });
                    }

                    let execution = execute_stub_tool(sandbox, &tool_call)?;
                    if !execution.board_ops.is_empty() {
                        board.apply_operations(&execution.board_ops);
                    }

                    conversation.push(ProviderMessage {
                        role: Role::ToolResult,
                        content: execution.output.clone(),
                        tool_calls: None,
                        tool_call_id: Some(tool_call.id.clone()),
                        attachments: Vec::new(),
                    });

                    if let Some((session_id, turn_id, tx)) = interactive_turn {
                        if !execution.board_ops.is_empty() {
                            let _ = tx.send(WorkerEvent::StepArtifact {
                                session_id: session_id.to_string(),
                                turn_id: turn_id.to_string(),
                                phase,
                                artifact: ExecutionArtifact::BoardOps {
                                    operations: execution.board_ops.clone(),
                                },
                            });
                            let _ = tx.send(WorkerEvent::BoardUpdate {
                                session_id: session_id.to_string(),
                                turn_id: turn_id.to_string(),
                                project_id: request.project_id.clone(),
                                operations: execution.board_ops.clone(),
                            });
                        }
                        let _ = tx.send(WorkerEvent::StepArtifact {
                            session_id: session_id.to_string(),
                            turn_id: turn_id.to_string(),
                            phase,
                            artifact: ExecutionArtifact::ToolResult {
                                tool_name: tool_call.name.clone(),
                                output: execution.output.clone(),
                            },
                        });
                    }

                    let assistant_note = format!(
                        "Stub {} phase used {} and observed: {}",
                        phase.as_str(),
                        tool_call.name,
                        execution.output
                    );
                    continuation_frames.push(build_continuation_frame(
                        phase,
                        &assistant_note,
                        vec![
                            ExecutionArtifact::ToolCall {
                                tool_name: tool_call.name.clone(),
                                args: tool_call.args.clone(),
                            },
                            ExecutionArtifact::ToolResult {
                                tool_name: tool_call.name.clone(),
                                output: execution.output.clone(),
                            },
                        ],
                    ));
                    compact_continuation_frames(&mut continuation_frames);
                    step_summary = assistant_note;
                } else {
                    let note = format!("Stub {} phase needed no tool call.", phase.as_str());
                    continuation_frames.push(build_continuation_frame(
                        phase,
                        &note,
                        vec![ExecutionArtifact::AssistantNote { text: note.clone() }],
                    ));
                    compact_continuation_frames(&mut continuation_frames);
                    step_summary = note.clone();
                    if let Some((session_id, turn_id, tx)) = interactive_turn {
                        let _ = tx.send(WorkerEvent::StepArtifact {
                            session_id: session_id.to_string(),
                            turn_id: turn_id.to_string(),
                            phase,
                            artifact: ExecutionArtifact::AssistantNote { text: note },
                        });
                    }
                }
            }
            AgentPhase::Respond => {
                final_response = stub_final_response(label, &last_user, &plan, &conversation);
                if let Some((session_id, turn_id, tx)) = interactive_turn {
                    stream_stub_delta_chunks(&final_response, session_id, turn_id, tx)?;
                }
                conversation.push(ProviderMessage {
                    role: Role::Assistant,
                    content: final_response.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                    attachments: Vec::new(),
                });
                step_summary = final_response.clone();
            }
        }

        if let Some((session_id, turn_id, tx)) = interactive_turn {
            let _ = tx.send(WorkerEvent::StepUpdate {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                phase,
                status: TurnStepStatus::Completed,
                summary: Some(step_summary),
            });
        }
    }

    Ok(final_response)
}

pub(crate) fn run_specialist_loop(
    auth: &CodexAuth,
    client: &CodexClient,
    sandbox: &Sandbox,
    request: &ProviderRequest,
    agent: AgentKind,
    label: &str,
    interactive_turn: Option<(&str, &str, &Sender<WorkerEvent>)>,
    background_job: Option<(&str, &str, &Sender<WorkerEvent>)>,
    is_subagent: bool,
) -> Result<String> {
    let mut conversation = request.messages.clone();
    let mut board = request
        .board
        .clone()
        .unwrap_or_default()
        .normalized_for_prompt();
    let mut total_tool_calls = 0u32;
    let mut continuation_frames: Vec<ContinuationFrame> = Vec::new();
    let mut final_response = String::new();

    for phase in AgentPhase::ALL {
        if let Some((session_id, turn_id, tx)) = interactive_turn {
            let _ = tx.send(WorkerEvent::PhaseChange {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                phase,
            });
            let _ = tx.send(WorkerEvent::StepUpdate {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                phase,
                status: TurnStepStatus::Running,
                summary: Some(phase.status().to_string()),
            });
            let _ = tx.send(WorkerEvent::ToolStatus {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                status: phase.status().to_string(),
            });
        }
        if let Some((session_id, job_id, tx)) = background_job {
            let _ = tx.send(WorkerEvent::JobUpdated {
                session_id: session_id.to_string(),
                job_id: job_id.to_string(),
                status: JobStatus::Running,
                summary: phase.status().to_string(),
            });
        }

        let phase_iterations = board.budgets.max_phase_iterations.max(1);
        let mut iterations = 0;
        loop {
            iterations += 1;
            board.apply_operations(&[BoardOperation::SetLastPhase { phase }]);

            let continuation_messages: Vec<ProviderMessage> = continuation_frames
                .iter()
                .map(|frame| ProviderMessage {
                    role: Role::System,
                    content: frame.as_system_text(),
                    tool_calls: None,
                    tool_call_id: None,
                    attachments: Vec::new(),
                })
                .collect();

            let instructions = build_specialist_instructions(request, &board, phase, agent, label);
            let allowed_tools = tools::tool_specs_for_agent_phase(agent, phase, is_subagent);
            let tool_choice = if allowed_tools.is_empty() {
                "none"
            } else {
                "auto"
            };

            let emit_text = matches!(phase, AgentPhase::Respond) && interactive_turn.is_some();
            let turn_key = interactive_turn
                .map(|(_, turn_id, _)| turn_id.to_string())
                .or_else(|| background_job.map(|(_, job_id, _)| job_id.to_string()))
                .unwrap_or_default();
            let session_key = interactive_turn
                .map(|(session_id, _, _)| session_id.to_string())
                .or_else(|| background_job.map(|(session_id, _, _)| session_id.to_string()))
                .unwrap_or_default();

            let turn_request = build_specialist_turn_request(
                request,
                &conversation,
                &continuation_messages,
                instructions,
                allowed_tools,
                tool_choice,
                emit_text,
            );

            let tx = interactive_turn
                .map(|(_, _, tx)| tx)
                .or_else(|| background_job.map(|(_, _, tx)| tx))
                .expect("coder loop requires a sender");
            let turn_response = client.execute_turn(
                auth,
                &turn_request,
                session_key.clone(),
                turn_key.clone(),
                tx,
            )?;
            let text_content = turn_response.text;
            let tool_calls = turn_response.tool_calls;

            let unique_calls = dedupe_tool_calls(tool_calls);
            let used_tools_this_iteration = !unique_calls.is_empty();
            if !unique_calls.is_empty() {
                let assistant_tool_calls = serde_json::json!(unique_calls
                    .iter()
                    .map(|tc| {
                        serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": serde_json::to_string(&tc.args).unwrap_or_else(|_| "null".to_string()),
                            }
                        })
                    })
                    .collect::<Vec<_>>());
                conversation.push(ProviderMessage {
                    role: Role::Assistant,
                    content: text_content.clone(),
                    tool_calls: Some(assistant_tool_calls.clone()),
                    tool_call_id: None,
                    attachments: Vec::new(),
                });

                if let Some((session_id, turn_id, tx)) = interactive_turn {
                    let _ = tx.send(WorkerEvent::ToolCalls {
                        session_id: session_id.to_string(),
                        turn_id: turn_id.to_string(),
                        calls: assistant_tool_calls,
                    });
                }

                let mut tool_summaries = Vec::new();
                let mut continuation_artifacts = Vec::new();
                for call in unique_calls {
                    continuation_artifacts.push(ExecutionArtifact::ToolCall {
                        tool_name: call.name.clone(),
                        args: call.args.clone(),
                    });
                    total_tool_calls = total_tool_calls.saturating_add(1);
                    if total_tool_calls > board.budgets.max_tool_calls {
                        anyhow::bail!("tool budget exceeded (>{})", board.budgets.max_tool_calls);
                    }

                    if let Some((session_id, turn_id, tx)) = interactive_turn {
                        let _ = tx.send(WorkerEvent::SystemNote {
                            session_id: session_id.to_string(),
                            turn_id: turn_id.to_string(),
                            note: tools::tool_slug(&call.name, &call.args),
                        });
                        let _ = tx.send(WorkerEvent::StepArtifact {
                            session_id: session_id.to_string(),
                            turn_id: turn_id.to_string(),
                            phase,
                            artifact: ExecutionArtifact::ToolCall {
                                tool_name: call.name.clone(),
                                args: call.args.clone(),
                            },
                        });
                    }

                    let tool_ctx = tools::SalsaToolContext {
                        sandbox,
                        auth,
                        client,
                        session_id: &session_key,
                        turn_id: &turn_key,
                        tx,
                        model: &request.model,
                        custom_prompt: request.custom_prompt.as_deref(),
                        is_subagent,
                    };
                    if let Some((session_id, job_id, tx)) = background_job {
                        let _ = tx.send(WorkerEvent::JobUpdated {
                            session_id: session_id.to_string(),
                            job_id: job_id.to_string(),
                            status: JobStatus::Running,
                            summary: format!("{}: {}", phase.status(), call.name),
                        });
                    }
                    let execution = tools::execute_tool(
                        &tool_ctx,
                        &call.name,
                        &call.args,
                        board.budgets.max_output_bytes,
                    );
                    if !execution.board_ops.is_empty() {
                        board.apply_operations(&execution.board_ops);
                    }
                    tool_summaries.push(format!(
                        "tool={} args={} output={}",
                        call.name,
                        serde_json::to_string(&call.args).unwrap_or_default(),
                        execution.output
                    ));
                    continuation_artifacts.push(ExecutionArtifact::ToolResult {
                        tool_name: call.name.clone(),
                        output: execution.output.clone(),
                    });

                    conversation.push(ProviderMessage {
                        role: Role::ToolResult,
                        content: execution.output.clone(),
                        tool_calls: None,
                        tool_call_id: Some(call.id.clone()),
                        attachments: Vec::new(),
                    });

                    if !execution.attachments.is_empty() {
                        conversation.push(ProviderMessage {
                            role: Role::System,
                            content: format!(
                                "Tool `{}` attached media from the workspace for inspection. Use the attachment directly when answering.",
                                call.name
                            ),
                            tool_calls: None,
                            tool_call_id: None,
                            attachments: execution.attachments.clone(),
                        });
                    }

                    if let Some((session_id, turn_id, tx)) = interactive_turn {
                        if !execution.board_ops.is_empty() {
                            let _ = tx.send(WorkerEvent::StepArtifact {
                                session_id: session_id.to_string(),
                                turn_id: turn_id.to_string(),
                                phase,
                                artifact: ExecutionArtifact::BoardOps {
                                    operations: execution.board_ops.clone(),
                                },
                            });
                            let _ = tx.send(WorkerEvent::BoardUpdate {
                                session_id: session_id.to_string(),
                                turn_id: turn_id.to_string(),
                                project_id: request.project_id.clone(),
                                operations: execution.board_ops.clone(),
                            });
                        }
                        let _ = tx.send(WorkerEvent::StepArtifact {
                            session_id: session_id.to_string(),
                            turn_id: turn_id.to_string(),
                            phase,
                            artifact: ExecutionArtifact::ToolResult {
                                tool_name: call.name.clone(),
                                output: execution.output.clone(),
                            },
                        });
                    }
                }

                let step_summary = compose_step_summary(&text_content, &tool_summaries);
                continuation_frames.push(build_continuation_frame(
                    phase,
                    &step_summary,
                    continuation_artifacts,
                ));
                compact_continuation_frames(&mut continuation_frames);
                if let Some((session_id, turn_id, tx)) = interactive_turn {
                    let _ = tx.send(WorkerEvent::StepUpdate {
                        session_id: session_id.to_string(),
                        turn_id: turn_id.to_string(),
                        phase,
                        status: TurnStepStatus::Completed,
                        summary: Some(step_summary.clone()),
                    });
                }
                if let Some((session_id, job_id, tx)) = background_job {
                    let _ = tx.send(WorkerEvent::JobUpdated {
                        session_id: session_id.to_string(),
                        job_id: job_id.to_string(),
                        status: JobStatus::Running,
                        summary: step_summary.clone(),
                    });
                }
            } else {
                let step_summary = compose_step_summary(&text_content, &[]);
                if matches!(phase, AgentPhase::Respond) {
                    final_response = text_content.clone();
                    conversation.push(ProviderMessage {
                        role: Role::Assistant,
                        content: text_content,
                        tool_calls: None,
                        tool_call_id: None,
                        attachments: Vec::new(),
                    });
                } else if !text_content.trim().is_empty() {
                    if let Some((session_id, turn_id, tx)) = interactive_turn {
                        let _ = tx.send(WorkerEvent::StepArtifact {
                            session_id: session_id.to_string(),
                            turn_id: turn_id.to_string(),
                            phase,
                            artifact: ExecutionArtifact::AssistantNote {
                                text: text_content.clone(),
                            },
                        });
                    }
                    continuation_frames.push(build_continuation_frame(
                        phase,
                        &step_summary,
                        vec![ExecutionArtifact::AssistantNote {
                            text: text_content.clone(),
                        }],
                    ));
                    compact_continuation_frames(&mut continuation_frames);
                }

                if let Some((session_id, turn_id, tx)) = interactive_turn {
                    let _ = tx.send(WorkerEvent::StepUpdate {
                        session_id: session_id.to_string(),
                        turn_id: turn_id.to_string(),
                        phase,
                        status: TurnStepStatus::Completed,
                        summary: Some(step_summary.clone()),
                    });
                }
                if let Some((session_id, job_id, tx)) = background_job {
                    let _ = tx.send(WorkerEvent::JobUpdated {
                        session_id: session_id.to_string(),
                        job_id: job_id.to_string(),
                        status: JobStatus::Running,
                        summary: step_summary.clone(),
                    });
                }
            }

            if should_advance_phase(
                phase,
                used_tools_this_iteration,
                iterations,
                phase_iterations,
            ) {
                break;
            }
        }
    }

    Ok(final_response.trim().to_string())
}

fn build_specialist_instructions(
    request: &ProviderRequest,
    board: &Board,
    phase: AgentPhase,
    agent: AgentKind,
    label: &str,
) -> String {
    let role_instructions = match agent {
        AgentKind::Coder => {
            "You are an expert software engineer assistant operating in a bounded phase loop.\n\
            The workspace tools are the only valid way to inspect or modify files."
        }
        AgentKind::Analyst => {
            "You are a specialized data analysis assistant operating in a bounded phase loop.\n\
            Use the dataframe tools to inspect datasets, compute statistics, and validate findings.\n\
            Do not modify workspace files or pretend to have run an analysis without a successful tool call."
        }
        _ => "You are a specialist assistant operating in a bounded phase loop.",
    };
    let phase_rules = match agent {
        AgentKind::Analyst => match phase {
            AgentPhase::Plan => {
                "Use only board_update. Define the analysis goal, target dataset, and the next question to answer."
            }
            AgentPhase::Explore => {
                "Inspect dataset files and gather evidence with read-only tools. Prefer dataframe inspection/statistics tools over guessing."
            }
            AgentPhase::Act => {
                "Run dataframe operations to answer the active analysis question. Do not call write, edit, delete, or shell tools."
            }
            AgentPhase::Verify => {
                "Cross-check findings with follow-up dataframe queries. Confirm assumptions such as nulls, row counts, and grouping logic."
            }
            AgentPhase::Respond => {
                "Do not call tools. Give a concise user-facing analysis summary with evidence and caveats."
            }
        },
        _ => phase.rules(),
    };
    let mut instructions = format!(
        "{role_instructions}\n\
        Never claim to have performed an action unless you actually called a tool and received success.\n\
        Use the board as authoritative planner state.\n\
        Specialist label: {label}.\n\
        Current phase: {}.\n\
        Phase rules:\n{}\n\
        Board state:\n{}",
        phase.as_str(),
        phase_rules,
        serde_yaml::to_string(board).unwrap_or_default()
    );

    if let Some(ref custom) = request.custom_prompt {
        instructions.push_str("\nUser custom prompt:\n");
        instructions.push_str(custom);
    }

    instructions
}

fn orchestrator_instructions(request: &ProviderRequest) -> String {
    let mut instructions = String::from(
        "You are the primary orchestrator assistant.\n\
        Stay conversational, clarify intent, and help the user shape plans when needed.\n\
        Do not claim to have edited files or run tools yourself.\n\
        If work is better handled by a specialist, tell the user briefly that you are delegating it."
    );
    if let Some(ref custom) = request.custom_prompt {
        instructions.push_str("\nUser custom prompt:\n");
        instructions.push_str(custom);
    }
    instructions
}

fn planner_instructions(request: &ProviderRequest) -> String {
    let mut instructions = String::from(
        "You are a planning specialist.\n\
        Work with the user to flesh out a plan when scope is unclear.\n\
        Produce concrete implementation steps, risks, and verification ideas.\n\
        Stay concise and actionable, and do not pretend work has already been executed.",
    );
    if let Some(ref custom) = request.custom_prompt {
        instructions.push_str("\nUser custom prompt:\n");
        instructions.push_str(custom);
    }
    instructions
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RouteDecision {
    Direct,
    Planner,
    Coder,
    Analyst,
}

fn route_request(request: &ProviderRequest) -> RouteDecision {
    let last_user = request
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, Role::User))
        .map(|message| message.content.trim().to_lowercase())
        .unwrap_or_default();

    if last_user.is_empty() {
        return RouteDecision::Direct;
    }

    let direct_markers = [
        "hi",
        "hello",
        "hey",
        "yo",
        "sup",
        "how are you",
        "what's up",
        "whats up",
        "good morning",
        "good afternoon",
        "good evening",
        "thanks",
        "thank you",
        "cool",
        "nice",
        "lol",
        "lmao",
    ];
    let planner_markers = [
        "plan",
        "roadmap",
        "strategy",
        "approach",
        "design",
        "architecture",
        "way forward",
        "break this down",
        "flesh out",
        "scope",
        "tradeoff",
        "trade-off",
        "what should we",
        "how should we",
        "before we",
        "thinking through",
        "brainstorm",
    ];
    let analyst_markers = [
        "dataframe",
        "dataset",
        "csv",
        "parquet",
        "polars",
        "statistics",
        "statistical",
        "correlation",
        "distribution",
        "group by",
        "groupby",
        "aggregate",
        "summary stats",
        "descriptive stats",
        "value counts",
        "mean",
        "median",
        "outlier",
    ];
    let coder_markers = [
        "fix",
        "build",
        "implement",
        "edit",
        "refactor",
        "debug",
        "investigate",
        "review",
        "search",
        "find",
        "open",
        "read",
        "write",
        "run",
        "create",
        "delete",
        "change",
        "update",
        "add",
        "remove",
        "file",
        "function",
        "module",
        "cargo",
        "compile",
        "failing",
        "broken",
        "bug",
        "error",
        "workspace",
        "repo",
        "codebase",
    ];
    let coder_phrases = [
        "run tests",
        "write code",
        "open the repo",
        "read the code",
        "check the codebase",
        "fix this",
        "implement this",
        "debug this",
        "review this code",
        "update the file",
        "change the file",
        "add a",
        "remove the",
        "search the repo",
        "test suite",
    ];
    let analyst_phrases = [
        "analyze this dataset",
        "analyze this csv",
        "look at the data",
        "summarize the dataset",
        "compute statistics",
        "run polars",
        "group the data",
        "column correlation",
    ];

    let stripped = last_user
        .trim_matches(|c: char| !c.is_alphanumeric() && !c.is_whitespace())
        .to_string();

    if stripped.split_whitespace().count() <= 3 && !last_user.contains('\n') && last_user.len() < 40
    {
        return RouteDecision::Direct;
    }

    if direct_markers.iter().any(|marker| {
        stripped == *marker
            || stripped.starts_with(&format!("{marker} "))
            || stripped.ends_with(&format!(" {marker}"))
    }) {
        return RouteDecision::Direct;
    }

    let analyst_score = marker_score(&last_user, &analyst_markers)
        + marker_score(&last_user, &analyst_phrases) * 2;
    let coder_score = marker_score(&last_user, &coder_markers)
        + marker_score(&last_user, &coder_phrases) * 2
        + if last_user.contains('\n') { 2 } else { 0 }
        + if last_user.len() > 160 { 1 } else { 0 };
    let planner_score = marker_score(&last_user, &planner_markers);

    if planner_score > 0 && coder_score == 0 {
        return RouteDecision::Planner;
    }
    if planner_score >= 2 && coder_score <= 2 {
        return RouteDecision::Planner;
    }
    if analyst_score > coder_score && analyst_score > 0 {
        return RouteDecision::Analyst;
    }
    if coder_score > 0 {
        return RouteDecision::Coder;
    }

    RouteDecision::Direct
}

fn marker_score(text: &str, markers: &[&str]) -> u32 {
    markers
        .iter()
        .filter(|marker| text.contains(**marker))
        .count() as u32
}

fn summarize_job_title(request: &ProviderRequest) -> String {
    let last_user = request
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, Role::User))
        .map(|message| message.content.trim())
        .unwrap_or("coding task");
    let first_line = last_user
        .lines()
        .next()
        .unwrap_or("coding task")
        .trim()
        .to_lowercase();
    let stopwords = [
        "the", "a", "an", "to", "for", "of", "and", "or", "that", "this", "can", "you", "please",
        "some", "just", "make", "with", "from", "into", "but",
    ];
    let mut parts = Vec::new();
    for word in first_line.split(|c: char| !c.is_alphanumeric()) {
        if word.len() < 2 || stopwords.contains(&word) {
            continue;
        }
        parts.push(word);
        if parts.len() == 5 {
            break;
        }
    }
    if parts.is_empty() {
        "coding-task".to_string()
    } else {
        parts.join("-")
    }
}

fn build_direct_turn_request(request: &ProviderRequest, instructions: String) -> ModelTurnRequest {
    let turn_messages = prepend_system_message(&request.messages, instructions);
    let chat_messages = build_chat_completion_messages(&turn_messages);
    ChatCompletionsRequestBuilder::direct(request.model.clone(), chat_messages).build()
}

fn build_specialist_turn_request(
    request: &ProviderRequest,
    conversation: &[ProviderMessage],
    continuation_messages: &[ProviderMessage],
    instructions: String,
    allowed_tools: Vec<serde_json::Value>,
    tool_choice: &str,
    emit_text_events: bool,
) -> ModelTurnRequest {
    let combined_messages: Vec<ProviderMessage> = conversation
        .iter()
        .chain(continuation_messages.iter())
        .cloned()
        .collect();
    let turn_messages = prepend_system_message(&combined_messages, instructions);
    let chat_messages = build_chat_completion_messages(&turn_messages);
    ChatCompletionsRequestBuilder::specialist(request.model.clone(), chat_messages, allowed_tools)
        .tool_choice(serde_json::json!(tool_choice))
        .emit_text_events(emit_text_events)
        .build()
}

fn build_chat_completion_messages(messages: &[ProviderMessage]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .map(ProviderMessage::as_chat_completion_message)
        .collect()
}

fn prepend_system_message(messages: &[ProviderMessage], instructions: String) -> Vec<ProviderMessage> {
    let mut turn_messages = Vec::with_capacity(messages.len() + 1);
    if !instructions.trim().is_empty() {
        turn_messages.push(ProviderMessage {
            role: Role::System,
            content: instructions,
            tool_calls: None,
            tool_call_id: None,
            attachments: Vec::new(),
        });
    }
    turn_messages.extend(messages.iter().cloned());
    turn_messages
}

fn dedupe_tool_calls(tool_calls: Vec<ToolCall>) -> Vec<ToolCall> {
    // The specialist loop stays transport-agnostic by consuming the canonical
    // `ToolCall { id, name, args }` representation regardless of provider.
    let mut calls_map: HashMap<String, ToolCall> = HashMap::new();
    for tc in tool_calls {
        let is_empty = tc.args.is_null()
            || (tc.args.is_object() && tc.args.as_object().is_some_and(|m| m.is_empty()));
        if !is_empty || !calls_map.contains_key(&tc.id) {
            calls_map.insert(tc.id.clone(), tc);
        }
    }
    calls_map.into_values().collect()
}

fn build_continuation_frame(
    phase: AgentPhase,
    summary: &str,
    artifacts: Vec<ExecutionArtifact>,
) -> ContinuationFrame {
    ContinuationFrame {
        phase,
        summary: summary.to_string(),
        artifacts,
    }
}

fn compact_continuation_frames(frames: &mut Vec<ContinuationFrame>) {
    const MAX_CONTINUATION_FRAMES: usize = 6;
    if frames.len() > MAX_CONTINUATION_FRAMES {
        let drain_count = frames.len() - MAX_CONTINUATION_FRAMES;
        frames.drain(0..drain_count);
    }
}

fn compose_step_summary(assistant_text: &str, tool_summaries: &[String]) -> String {
    let trimmed = assistant_text.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    if !tool_summaries.is_empty() {
        return format!("{} tool actions", tool_summaries.len());
    }
    "completed".to_string()
}

fn should_advance_phase(
    phase: AgentPhase,
    used_tools_this_iteration: bool,
    iterations: u32,
    max_iterations: u32,
) -> bool {
    if iterations >= max_iterations {
        return true;
    }
    match phase {
        AgentPhase::Plan => true,
        AgentPhase::Explore | AgentPhase::Act | AgentPhase::Verify => !used_tools_this_iteration,
        AgentPhase::Respond => true,
    }
}

#[derive(Debug, Clone)]
struct StubToolCall {
    id: String,
    name: String,
    args: serde_json::Value,
}

impl StubToolCall {
    fn as_chat_completion_call(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "type": "function",
            "function": {
                "name": self.name,
                "arguments": serde_json::to_string(&self.args).unwrap_or_else(|_| "null".to_string()),
            }
        })
    }
}

#[derive(Debug, Clone)]
struct StubToolPlan {
    explore: Option<StubPhaseCall>,
    act: Option<StubPhaseCall>,
    verify: Option<StubPhaseCall>,
}

impl StubToolPlan {
    fn from_prompt(prompt: &str, agent: AgentKind) -> Self {
        let lowered = prompt.to_lowercase();
        let mut plan = if lowered.contains("write") {
            Self {
                explore: Some(StubPhaseCall::list_workspace()),
                act: Some(StubPhaseCall::write_note()),
                verify: Some(StubPhaseCall::read_note()),
            }
        } else if lowered.contains("read") {
            Self {
                explore: Some(StubPhaseCall::list_workspace()),
                act: Some(StubPhaseCall::read_note()),
                verify: Some(StubPhaseCall::list_workspace()),
            }
        } else if lowered.contains("test") || lowered.contains("cargo") {
            Self {
                explore: Some(StubPhaseCall::list_workspace()),
                act: Some(StubPhaseCall::run_tests()),
                verify: Some(StubPhaseCall::list_workspace()),
            }
        } else if lowered.contains("list") || lowered.contains("files") {
            Self {
                explore: Some(StubPhaseCall::list_workspace()),
                act: Some(StubPhaseCall::read_note()),
                verify: None,
            }
        } else {
            Self {
                explore: None,
                act: None,
                verify: None,
            }
        };

        if matches!(agent, AgentKind::Analyst) && plan.explore.is_none() {
            plan.explore = Some(StubPhaseCall::list_workspace());
        }

        plan
    }

    fn has_tools(&self) -> bool {
        self.explore.is_some() || self.act.is_some() || self.verify.is_some()
    }

    fn call_for_phase(&self, phase: AgentPhase) -> Option<&StubPhaseCall> {
        match phase {
            AgentPhase::Explore => self.explore.as_ref(),
            AgentPhase::Act => self.act.as_ref(),
            AgentPhase::Verify => self.verify.as_ref(),
            AgentPhase::Plan | AgentPhase::Respond => None,
        }
    }
}

#[derive(Debug, Clone)]
struct StubPhaseCall {
    tool_name: &'static str,
    args: serde_json::Value,
    description: &'static str,
}

impl StubPhaseCall {
    fn list_workspace() -> Self {
        Self {
            tool_name: "fs_list",
            args: serde_json::json!({ "path": "." }),
            description: "inspect the workspace tree",
        }
    }

    fn read_note() -> Self {
        Self {
            tool_name: "fs_read",
            args: serde_json::json!({ "path": "stub-note.txt" }),
            description: "read a deterministic workspace file",
        }
    }

    fn write_note() -> Self {
        Self {
            tool_name: "fs_write",
            args: serde_json::json!({
                "path": "stub-note.txt",
                "content": "stub provider wrote this file for migration validation\n"
            }),
            description: "write a deterministic workspace file",
        }
    }

    fn run_tests() -> Self {
        Self {
            tool_name: "sh_run",
            args: serde_json::json!({ "command": "cargo test --quiet" }),
            description: "run a deterministic validation command",
        }
    }

    fn to_tool_call(&self, phase: AgentPhase) -> StubToolCall {
        StubToolCall {
            id: format!("stub-{}-{}", phase.as_str().to_lowercase(), self.tool_name),
            name: self.tool_name.to_string(),
            args: self.args.clone(),
        }
    }

    fn assistant_preface(&self, phase: AgentPhase) -> String {
        format!(
            "Stub {} phase will {}.",
            phase.as_str().to_lowercase(),
            self.description
        )
    }
}

struct StubExecution {
    output: String,
    board_ops: Vec<BoardOperation>,
}

fn latest_user_message(messages: &[ProviderMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, Role::User))
        .map(|message| message.content.clone())
        .unwrap_or_default()
}

fn stub_plain_response(request: &ProviderRequest) -> String {
    let prompt = latest_user_message(&request.messages);
    if prompt.trim().is_empty() {
        "Stub provider is ready for migration testing.".to_string()
    } else {
        format!(
            "Stub provider handled this without tools: {}",
            prompt.trim()
        )
    }
}

fn stub_phase_note(label: &str, phase: AgentPhase, prompt: &str, plan: &StubToolPlan) -> String {
    if plan.has_tools() {
        format!(
            "Stub {label} plan for {} phase: use deterministic tool calls for `{}`.",
            phase.as_str(),
            prompt.trim()
        )
    } else {
        format!(
            "Stub {label} plan for {} phase: answer directly without tool usage.",
            phase.as_str()
        )
    }
}

fn stub_final_response(
    label: &str,
    prompt: &str,
    plan: &StubToolPlan,
    conversation: &[ProviderMessage],
) -> String {
    let tool_outputs: Vec<&str> = conversation
        .iter()
        .filter(|message| matches!(message.role, Role::ToolResult))
        .map(|message| message.content.trim())
        .filter(|content| !content.is_empty())
        .collect();

    if !tool_outputs.is_empty() {
        format!(
            "Stub {label} completed `{}` using {} deterministic tool step(s). Latest tool output: {}",
            prompt.trim(),
            tool_outputs.len(),
            tool_outputs.last().copied().unwrap_or("completed")
        )
    } else if plan.has_tools() {
        format!(
            "Stub {label} completed `{}` with the deterministic plan but no tool output was produced.",
            prompt.trim()
        )
    } else {
        format!(
            "Stub {label} answered `{}` directly without tools.",
            prompt.trim()
        )
    }
}

fn stream_stub_delta_chunks(
    reply: &str,
    session_id: &str,
    turn_id: &str,
    tx: &Sender<WorkerEvent>,
) -> Result<()> {
    for chunk in reply.split_inclusive(' ') {
        thread::sleep(Duration::from_millis(20));
        tx.send(WorkerEvent::Delta {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            delta: chunk.to_string(),
        })
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    }
    Ok(())
}

fn execute_stub_tool(sandbox: &Sandbox, tool_call: &StubToolCall) -> Result<StubExecution> {
    match tool_call.name.as_str() {
        "fs_list" => {
            let target = tool_call
                .args
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or(".");
            let root = sandbox.resolve(target)?;
            let mut entries: Vec<String> = std::fs::read_dir(root)?
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .collect();
            entries.sort();
            Ok(StubExecution {
                output: if entries.is_empty() {
                    "workspace is empty".to_string()
                } else {
                    format!("workspace entries: {}", entries.join(", "))
                },
                board_ops: Vec::new(),
            })
        }
        "fs_read" => {
            let target = tool_call
                .args
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or("stub-note.txt");
            let path = sandbox.resolve(target)?;
            let output = std::fs::read_to_string(path)
                .unwrap_or_else(|_| "stub-note.txt is not present yet".to_string());
            Ok(StubExecution {
                output,
                board_ops: Vec::new(),
            })
        }
        "fs_write" => {
            let target = tool_call
                .args
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or("stub-note.txt");
            let content = tool_call
                .args
                .get("content")
                .and_then(|value| value.as_str())
                .unwrap_or("stub provider wrote this file\n");
            let path = sandbox.resolve(target)?;
            std::fs::write(path, content)?;
            Ok(StubExecution {
                output: format!("wrote {}", target),
                board_ops: vec![BoardOperation::AddFact {
                    fact: format!("stub wrote {}", target),
                }],
            })
        }
        "sh_run" => {
            let command = tool_call
                .args
                .get("command")
                .and_then(|value| value.as_str())
                .unwrap_or("cargo test --quiet");
            let output = if cfg!(target_os = "windows") {
                std::process::Command::new("cmd")
                    .args(["/C", command])
                    .current_dir(&sandbox.root)
                    .output()?
            } else {
                std::process::Command::new("sh")
                    .args(["-c", command])
                    .current_dir(&sandbox.root)
                    .output()?
            };
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            Ok(StubExecution {
                output: format!("{}{}", stdout, stderr).trim().to_string(),
                board_ops: vec![BoardOperation::AddFact {
                    fact: format!("stub ran {}", command),
                }],
            })
        }
        other => Ok(StubExecution {
            output: format!("stub provider does not implement {}", other),
            board_ops: Vec::new(),
        }),
    }
}
