use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use anyhow::Result;

use crate::agent::{ProviderMessage, ProviderRequest, WorkerEvent};
use crate::api::codex::{CodexClient, ToolCall};
use crate::auth::CodexAuth;
use crate::models::{
    AgentPhase, Board, BoardOperation, ContinuationFrame, ExecutionArtifact, Role, TurnStepStatus,
};
use crate::tools::{self, Sandbox};

pub trait Provider: std::fmt::Debug + Send + 'static {
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
        "codex"
    }

    fn generate(
        &self,
        request: &ProviderRequest,
        session_id: String,
        turn_id: String,
        tx: &Sender<WorkerEvent>,
    ) {
        let mut conversation = request.messages.clone();
        let mut board = request.board.clone().unwrap_or_default().normalized_for_prompt();
        let mut total_tool_calls = 0u32;
        let full_loop = should_run_full_loop(request);
        let mut continuation_frames: Vec<ContinuationFrame> = Vec::new();

        for &phase in phases_for_turn(full_loop) {
            let _ = tx.send(WorkerEvent::PhaseChange {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                phase,
            });
            let _ = tx.send(WorkerEvent::StepUpdate {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                phase,
                status: TurnStepStatus::Running,
                summary: Some(phase_status(phase).to_string()),
            });

            let _ = tx.send(WorkerEvent::ToolStatus {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                status: phase_status(phase).to_string(),
            });

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
                    })
                    .collect();
                let input: Vec<serde_json::Value> = conversation
                    .iter()
                    .chain(continuation_messages.iter())
                    .map(|m| m.as_json())
                    .collect();

                let instructions = build_instructions(request, &board, phase, full_loop);
                let allowed_tools = tools::tool_specs_for_phase(phase);
                let tool_choice = if allowed_tools.is_empty() { "none" } else { "auto" };

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

                let (text_content, tool_calls) = match self.client.request(
                    &self.auth,
                    &body,
                    session_id.clone(),
                    turn_id.clone(),
                    matches!(phase, AgentPhase::Respond),
                    tx,
                ) {
                    Ok(res) => res,
                    Err(e) => {
                        let _ = tx.send(WorkerEvent::StepUpdate {
                            session_id: session_id.clone(),
                            turn_id: turn_id.clone(),
                            phase,
                            status: TurnStepStatus::Failed,
                            summary: Some(e.to_string()),
                        });
                        let _ = tx.send(WorkerEvent::Error {
                            session_id: session_id.clone(),
                            turn_id: turn_id.clone(),
                            err: e.to_string(),
                        });
                        return;
                    }
                };

                let unique_calls = dedupe_tool_calls(tool_calls);
                let used_tools_this_iteration = !unique_calls.is_empty();
                if !unique_calls.is_empty() {
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
                        session_id: session_id.clone(),
                        turn_id: turn_id.clone(),
                        calls: calls_json.clone(),
                    });
                    for call in &unique_calls {
                        let _ = tx.send(WorkerEvent::StepArtifact {
                            session_id: session_id.clone(),
                            turn_id: turn_id.clone(),
                            phase,
                            artifact: ExecutionArtifact::ToolCall {
                                tool_name: call.name.clone(),
                                args: call.args.clone(),
                            },
                        });
                    }

                    let mut tool_outputs = Vec::new();
                    let mut tool_summaries = Vec::new();
                    let mut continuation_artifacts = Vec::new();
                    for call in unique_calls {
                        continuation_artifacts.push(ExecutionArtifact::ToolCall {
                            tool_name: call.name.clone(),
                            args: call.args.clone(),
                        });
                        total_tool_calls = total_tool_calls.saturating_add(1);
                        if total_tool_calls > board.budgets.max_tool_calls {
                            let _ = tx.send(WorkerEvent::StepUpdate {
                                session_id: session_id.clone(),
                                turn_id: turn_id.clone(),
                                phase,
                                status: TurnStepStatus::Failed,
                                summary: Some(format!(
                                    "tool budget exceeded (>{})",
                                    board.budgets.max_tool_calls
                                )),
                            });
                            let _ = tx.send(WorkerEvent::Error {
                                session_id: session_id.clone(),
                                turn_id: turn_id.clone(),
                                err: format!(
                                    "tool budget exceeded (>{})",
                                    board.budgets.max_tool_calls
                                ),
                            });
                            return;
                        }

                        let _ = tx.send(WorkerEvent::SystemNote {
                            session_id: session_id.clone(),
                            turn_id: turn_id.clone(),
                            note: tools::tool_slug(&call.name, &call.args),
                        });

                        let (output, board_ops) = tools::execute_tool(
                            &self.sandbox,
                            &call.name,
                            &call.args,
                            board.budgets.max_output_bytes,
                        );

                        if !board_ops.is_empty() {
                            board.apply_operations(&board_ops);
                            continuation_artifacts.push(ExecutionArtifact::BoardOps {
                                operations: board_ops.clone(),
                            });
                            let _ = tx.send(WorkerEvent::StepArtifact {
                                session_id: session_id.clone(),
                                turn_id: turn_id.clone(),
                                phase,
                                artifact: ExecutionArtifact::BoardOps {
                                    operations: board_ops.clone(),
                                },
                            });
                            let _ = tx.send(WorkerEvent::BoardUpdate {
                                session_id: session_id.clone(),
                                turn_id: turn_id.clone(),
                                project_id: request.project_id.clone(),
                                operations: board_ops,
                            });
                        }

                        tool_summaries.push(format!(
                            "tool={} args={} output={}",
                            call.name,
                            serde_json::to_string(&call.args).unwrap_or_default(),
                            output
                        ));
                        continuation_artifacts.push(ExecutionArtifact::ToolResult {
                            tool_name: call.name.clone(),
                            output: output.clone(),
                        });
                        let _ = tx.send(WorkerEvent::StepArtifact {
                            session_id: session_id.clone(),
                            turn_id: turn_id.clone(),
                            phase,
                            artifact: ExecutionArtifact::ToolResult {
                                tool_name: call.name.clone(),
                                output: output.clone(),
                            },
                        });
                        tool_outputs.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": call.id,
                            "output": output,
                        }));
                    }

                    let result_content = serde_json::to_string(&tool_outputs).unwrap_or_default();
                    let _ = tx.send(WorkerEvent::ToolResult {
                        session_id: session_id.clone(),
                        turn_id: turn_id.clone(),
                        content: result_content.clone(),
                    });

                    let step_summary = compose_step_summary(&text_content, &tool_summaries);
                    continuation_frames.push(build_continuation_frame(
                        phase,
                        &step_summary,
                        continuation_artifacts,
                    ));
                    compact_continuation_frames(&mut continuation_frames);
                    let _ = tx.send(WorkerEvent::StepUpdate {
                        session_id: session_id.clone(),
                        turn_id: turn_id.clone(),
                        phase,
                        status: TurnStepStatus::Completed,
                        summary: Some(step_summary),
                    });
                } else {
                    let step_summary = compose_step_summary(&text_content, &[]);
                    if matches!(phase, AgentPhase::Respond) {
                        conversation.push(ProviderMessage {
                            role: Role::Assistant,
                            content: text_content,
                            tool_calls: None,
                        });
                    } else if !text_content.trim().is_empty() {
                        let _ = tx.send(WorkerEvent::StepArtifact {
                            session_id: session_id.clone(),
                            turn_id: turn_id.clone(),
                            phase,
                            artifact: ExecutionArtifact::AssistantNote {
                                text: text_content.clone(),
                            },
                        });
                        continuation_frames.push(build_continuation_frame(
                            phase,
                            &step_summary,
                            vec![ExecutionArtifact::AssistantNote {
                                text: text_content.clone(),
                            }],
                        ));
                        compact_continuation_frames(&mut continuation_frames);
                    }
                    let _ = tx.send(WorkerEvent::StepUpdate {
                        session_id: session_id.clone(),
                        turn_id: turn_id.clone(),
                        phase,
                        status: TurnStepStatus::Completed,
                        summary: Some(step_summary),
                    });
                }

                if should_advance_phase(phase, used_tools_this_iteration, iterations, phase_iterations) {
                    break;
                }
            }
        }

        let _ = tx.send(WorkerEvent::Done {
            session_id,
            turn_id,
        });
    }
}

fn build_instructions(
    request: &ProviderRequest,
    board: &Board,
    phase: AgentPhase,
    full_loop: bool,
) -> String {
    let mut instructions = if full_loop {
        format!(
            "You are an expert software engineer assistant operating in a bounded phase loop.\n\
            The workspace tools are the only valid way to inspect or modify files.\n\
            Never claim to have performed an action unless you actually called a tool and received success.\n\
            Use the board as authoritative planner state.\n\
            Current phase: {}.\n\
            Phase rules:\n{}\n\
            Board state:\n{}",
            phase_name(phase),
            phase_rules(phase),
            serde_yaml::to_string(board).unwrap_or_default()
        )
    } else {
        String::from(
            "You are in direct response mode.\n\
            The user sent a conversational message, not a task request.\n\
            Reply naturally and briefly. Do not plan, do not mention phases, and do not call tools unless absolutely necessary."
        )
    };

    if let Some(ref custom) = request.custom_prompt {
        instructions.push_str("\nUser custom prompt:\n");
        instructions.push_str(custom);
    }

    instructions
}

fn phase_name(phase: AgentPhase) -> &'static str {
    match phase {
        AgentPhase::Plan => "plan",
        AgentPhase::Explore => "explore",
        AgentPhase::Act => "act",
        AgentPhase::Verify => "verify",
        AgentPhase::Respond => "respond",
    }
}

fn phase_rules(phase: AgentPhase) -> &'static str {
    match phase {
        AgentPhase::Plan => {
            "Use only board_update. Set or refine the goal, summary, tasks, and current task before any work."
        }
        AgentPhase::Explore => {
            "Gather evidence with read-only tools. Update the board with facts, blockers, and task state. Do not modify files."
        }
        AgentPhase::Act => {
            "Execute the selected task. Prefer narrow edits. Record attempts and task status changes in the board."
        }
        AgentPhase::Verify => {
            "Validate the work with focused reads or commands. Add evidence to the board and mark tasks done or blocked."
        }
        AgentPhase::Respond => {
            "Do not call tools. Give a concise user-facing summary of what changed, what was verified, and what remains."
        }
    }
}

fn phase_status(phase: AgentPhase) -> &'static str {
    match phase {
        AgentPhase::Plan => "planning...",
        AgentPhase::Explore => "exploring...",
        AgentPhase::Act => "executing...",
        AgentPhase::Verify => "verifying...",
        AgentPhase::Respond => "responding...",
    }
}

fn phases_for_turn(full_loop: bool) -> &'static [AgentPhase] {
    if full_loop {
        &[
            AgentPhase::Plan,
            AgentPhase::Explore,
            AgentPhase::Act,
            AgentPhase::Verify,
            AgentPhase::Respond,
        ]
    } else {
        &[AgentPhase::Respond]
    }
}

fn should_run_full_loop(request: &ProviderRequest) -> bool {
    let last_user = request
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, Role::User))
        .map(|message| message.content.trim().to_lowercase())
        .unwrap_or_default();

    if last_user.is_empty() {
        return false;
    }

    if last_user.contains('\n') {
        return true;
    }

    if last_user.len() > 160 {
        return true;
    }

    let task_markers = [
        "fix", "build", "implement", "edit", "refactor", "debug", "investigate", "analyze",
        "review", "search", "find", "open", "read", "write", "run", "test", "create", "delete",
        "change", "update", "add", "remove", "why is", "how do i", "can you", "please", "need you to",
    ];
    if task_markers.iter().any(|marker| last_user.contains(marker)) {
        return true;
    }

    let chat_markers = [
        "hi", "hello", "hey", "yo", "sup", "how are you", "what's up", "whats up",
        "good morning", "good afternoon", "good evening", "thanks", "thank you", "cool",
        "nice", "lol", "lmao",
    ];

    let stripped = last_user
        .trim_matches(|c: char| !c.is_alphanumeric() && !c.is_whitespace())
        .to_string();

    if chat_markers
        .iter()
        .any(|marker| stripped == *marker || stripped.starts_with(&format!("{marker} ")) || stripped.ends_with(&format!(" {marker}")))
    {
        return false;
    }

    false
}

fn dedupe_tool_calls(tool_calls: Vec<ToolCall>) -> Vec<ToolCall> {
    let mut calls_map: HashMap<String, ToolCall> = HashMap::new();
    for tc in tool_calls {
        let is_empty =
            tc.args.is_null() || (tc.args.is_object() && tc.args.as_object().is_some_and(|m| m.is_empty()));
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
