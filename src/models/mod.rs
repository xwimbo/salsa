use serde::{Deserialize, Serialize};
use serde_json;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Role {
    User,
    Assistant,
    System,
    ToolResult,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub body: String,
    #[serde(default)]
    pub tool_calls: Option<serde_json::Value>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub messages: Vec<Message>,
    #[serde(skip)]
    pub input: String,
    #[serde(skip)]
    pub pending: bool,
    #[serde(skip)]
    pub scroll: u16,
    #[serde(skip)]
    pub pending_turn_id: Option<String>,
    #[serde(skip)]
    pub pending_project_id: Option<String>,
    #[serde(skip)]
    pub turn_steps: Vec<TurnStep>,
    #[serde(default)]
    pub jobs: Vec<BackgroundJob>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Orchestrator,
    Planner,
    Coder,
    Analyst,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackgroundJob {
    pub id: String,
    pub agent: AgentKind,
    pub title: String,
    pub status: JobStatus,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub summary: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStepStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionArtifact {
    AssistantNote {
        text: String,
    },
    ToolCall {
        tool_name: String,
        args: serde_json::Value,
    },
    ToolResult {
        tool_name: String,
        output: String,
    },
    BoardOps {
        operations: Vec<BoardOperation>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TurnStep {
    pub phase: AgentPhase,
    pub status: TurnStepStatus,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub artifacts: Vec<ExecutionArtifact>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContinuationFrame {
    pub phase: AgentPhase,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub artifacts: Vec<ExecutionArtifact>,
}

impl ContinuationFrame {
    pub fn as_system_text(&self) -> String {
        let mut content = format!(
            "Internal continuation frame.\nphase={}\nsummary={}",
            self.phase.as_str(),
            self.summary.trim()
        );

        if !self.artifacts.is_empty() {
            content.push_str("\nartifacts:\n");
            for artifact in &self.artifacts {
                content.push_str("- ");
                content.push_str(&artifact.describe());
                content.push('\n');
            }
        }
        content
    }
}

impl ExecutionArtifact {
    pub fn describe(&self) -> String {
        match self {
            ExecutionArtifact::AssistantNote { text } => format!("assistant_note={}", text.trim()),
            ExecutionArtifact::ToolCall { tool_name, args } => format!(
                "tool_call name={} args={}",
                tool_name,
                serde_json::to_string(args).unwrap_or_default()
            ),
            ExecutionArtifact::ToolResult { tool_name, output } => {
                format!("tool_result name={} output={}", tool_name, output)
            }
            ExecutionArtifact::BoardOps { operations } => format!(
                "board_ops={}",
                serde_json::to_string(operations).unwrap_or_default()
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Todo,
    InProgress,
    Done,
    Blocked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPhase {
    Plan,
    Explore,
    Act,
    Verify,
    Respond,
}

impl AgentPhase {
    pub const ALL: [Self; 5] = [
        Self::Plan,
        Self::Explore,
        Self::Act,
        Self::Verify,
        Self::Respond,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Explore => "explore",
            Self::Act => "act",
            Self::Verify => "verify",
            Self::Respond => "respond",
        }
    }

    pub fn status(self) -> &'static str {
        match self {
            Self::Plan => "planning...",
            Self::Explore => "exploring...",
            Self::Act => "executing...",
            Self::Verify => "verifying...",
            Self::Respond => "responding...",
        }
    }

    pub fn rules(self) -> &'static str {
        match self {
            Self::Plan => {
                "Use only board_update. Set or refine the goal, summary, tasks, and current task before any work."
            }
            Self::Explore => {
                "Gather evidence with read-only tools. Update the board with facts, blockers, and task state. Do not modify files."
            }
            Self::Act => {
                "Execute the selected task. Prefer narrow edits. Record attempts and task status changes in the board."
            }
            Self::Verify => {
                "Validate the work with focused reads or commands. Add evidence to the board and mark tasks done or blocked."
            }
            Self::Respond => {
                "Do not call tools. Give a concise user-facing summary of what changed, what was verified, and what remains."
            }
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub status: TaskStatus,
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub attempt_count: u32,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct TurnBudget {
    pub max_phase_iterations: u32,
    pub max_tool_calls: u32,
    pub max_output_bytes: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Board {
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub current_task_id: Option<String>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub facts: Vec<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub budgets: TurnBudget,
    #[serde(default)]
    pub last_phase: Option<AgentPhase>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BoardOperation {
    SetGoal {
        goal: String,
    },
    SetSummary {
        summary: String,
    },
    SetCurrentTask {
        task_id: Option<String>,
    },
    AddTask {
        id: String,
        title: String,
        #[serde(default)]
        deps: Vec<String>,
        #[serde(default)]
        acceptance_criteria: Vec<String>,
    },
    UpdateTaskStatus {
        task_id: String,
        status: TaskStatus,
    },
    RecordAttempt {
        task_id: String,
        #[serde(default)]
        last_error: Option<String>,
    },
    AddTaskEvidence {
        task_id: String,
        evidence: String,
    },
    AddFact {
        fact: String,
    },
    AddBlocker {
        blocker: String,
    },
    ClearBlockers,
    SetBudget {
        #[serde(default)]
        max_phase_iterations: Option<u32>,
        #[serde(default)]
        max_tool_calls: Option<u32>,
        #[serde(default)]
        max_output_bytes: Option<usize>,
    },
    SetLastPhase {
        phase: AgentPhase,
    },
}

impl Board {
    pub fn normalized_for_prompt(&self) -> Self {
        let mut board = self.clone();
        if board.budgets.max_phase_iterations == 0 {
            board.budgets.max_phase_iterations = 2;
        }
        if board.budgets.max_tool_calls == 0 {
            board.budgets.max_tool_calls = 8;
        }
        if board.budgets.max_output_bytes == 0 {
            board.budgets.max_output_bytes = 12_000;
        }
        board
    }

    pub fn apply_operations(&mut self, operations: &[BoardOperation]) {
        for op in operations {
            match op {
                BoardOperation::SetGoal { goal } => self.goal = goal.clone(),
                BoardOperation::SetSummary { summary } => self.summary = summary.clone(),
                BoardOperation::SetCurrentTask { task_id } => {
                    self.current_task_id = task_id.clone();
                }
                BoardOperation::AddTask {
                    id,
                    title,
                    deps,
                    acceptance_criteria,
                } => {
                    if !self.tasks.iter().any(|task| task.id == *id) {
                        self.tasks.push(Task {
                            id: id.clone(),
                            title: title.clone(),
                            status: TaskStatus::Todo,
                            deps: deps.clone(),
                            acceptance_criteria: acceptance_criteria.clone(),
                            evidence: Vec::new(),
                            attempt_count: 0,
                            last_error: None,
                        });
                    }
                }
                BoardOperation::UpdateTaskStatus { task_id, status } => {
                    if let Some(task) = self.tasks.iter_mut().find(|task| task.id == *task_id) {
                        task.status = *status;
                    }
                }
                BoardOperation::RecordAttempt {
                    task_id,
                    last_error,
                } => {
                    if let Some(task) = self.tasks.iter_mut().find(|task| task.id == *task_id) {
                        task.attempt_count = task.attempt_count.saturating_add(1);
                        task.last_error = last_error.clone();
                    }
                }
                BoardOperation::AddTaskEvidence { task_id, evidence } => {
                    if let Some(task) = self.tasks.iter_mut().find(|task| task.id == *task_id) {
                        task.evidence.push(evidence.clone());
                    }
                }
                BoardOperation::AddFact { fact } => self.facts.push(fact.clone()),
                BoardOperation::AddBlocker { blocker } => self.blockers.push(blocker.clone()),
                BoardOperation::ClearBlockers => self.blockers.clear(),
                BoardOperation::SetBudget {
                    max_phase_iterations,
                    max_tool_calls,
                    max_output_bytes,
                } => {
                    if let Some(value) = max_phase_iterations {
                        self.budgets.max_phase_iterations = *value;
                    }
                    if let Some(value) = max_tool_calls {
                        self.budgets.max_tool_calls = *value;
                    }
                    if let Some(value) = max_output_bytes {
                        self.budgets.max_output_bytes = *value;
                    }
                }
                BoardOperation::SetLastPhase { phase } => self.last_phase = Some(*phase),
            }
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub board: Board,
    pub prompt: Option<String>,
}
