use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use uuid::Uuid;

use crate::agent::{ProviderMessage, ProviderRequest, WorkerEvent};
use crate::api::codex::{CodexClient, ToolCall};
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

#[derive(Debug)]
pub struct CodexProvider {
    auth: CodexAuth,
    client: CodexClient,
    sandbox: Sandbox,
}

impl CodexProvider {
    pub fn new(auth: CodexAuth, workspace: PathBuf) -> Result<Self> {
        let client = CodexClient::new()?;
        let sandbox = Sandbox::new(workspace)?;
        Ok(Self {
            auth,
            client,
            sandbox,
        })
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
        match request.agent {
            AgentKind::Coder => {
                if let Err(err) = run_specialist_turn(
                    &self.auth,
                    &self.client,
                    &self.sandbox,
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
                    &self.auth,
                    &self.client,
                    &self.sandbox,
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
                    &self.auth,
                    &self.client,
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
                        &self.auth,
                        &self.client,
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
                        &self.auth,
                        &self.client,
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
                        &self.auth,
                        &self.client,
                        &self.sandbox,
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
                        &self.auth,
                        &self.client,
                        &self.sandbox,
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
}

fn spawn_background_specialist(
    auth: &CodexAuth,
    client: &CodexClient,
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
    let client = client.clone();
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
            &client,
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
    client: &CodexClient,
    request: &ProviderRequest,
    session_id: String,
    turn_id: String,
    tx: &Sender<WorkerEvent>,
    instructions: String,
) -> Result<()> {
    let input: Vec<serde_json::Value> = request.messages.iter().map(|m| m.as_json()).collect();
    let body = serde_json::json!({
        "model": request.model,
        "instructions": instructions,
        "input": input,
        "tools": [],
        "tool_choice": "none",
        "parallel_tool_calls": false,
        "reasoning": null,
        "store": false,
        "stream": true,
    });

    client.request(auth, &body, session_id.clone(), turn_id.clone(), true, tx)?;
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
                    attachments: Vec::new(),
                })
                .collect();
            let input: Vec<serde_json::Value> = conversation
                .iter()
                .chain(continuation_messages.iter())
                .map(|m| m.as_json())
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

            let body = serde_json::json!({
                "model": request.model,
                "instructions": instructions,
                "input": input,
                "tools": allowed_tools,
                "tool_choice": tool_choice,
                "parallel_tool_calls": false,
                "reasoning": null,
                "store": false,
                "stream": true,
            });

            let tx = interactive_turn
                .map(|(_, _, tx)| tx)
                .or_else(|| background_job.map(|(_, _, tx)| tx))
                .expect("coder loop requires a sender");
            let (text_content, tool_calls) =
                client.request(auth, &body, session_key.clone(), turn_key.clone(), emit_text, tx)?;

            let unique_calls = dedupe_tool_calls(tool_calls);
            let used_tools_this_iteration = !unique_calls.is_empty();
            if !unique_calls.is_empty() {
                if let Some((session_id, turn_id, tx)) = interactive_turn {
                    let calls_json = serde_json::json!(unique_calls
                        .iter()
                        .map(|tc| {
                            serde_json::json!({
                                "id": tc.id,
                                "function": {
                                    "name": tc.name,
                                    "arguments": tc.args,
                                }
                            })
                        })
                        .collect::<Vec<_>>());
                    let _ = tx.send(WorkerEvent::ToolCalls {
                        session_id: session_id.to_string(),
                        turn_id: turn_id.to_string(),
                        calls: calls_json,
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

                    if !execution.attachments.is_empty() {
                        conversation.push(ProviderMessage {
                            role: Role::ToolResult,
                            content: format!(
                                "Tool `{}` attached media from the workspace for inspection. Use the attachment directly when answering.",
                                call.name
                            ),
                            tool_calls: None,
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
                        summary: Some(step_summary),
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
                        summary: Some(step_summary),
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

fn dedupe_tool_calls(tool_calls: Vec<ToolCall>) -> Vec<ToolCall> {
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
