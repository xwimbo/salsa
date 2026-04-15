# Agent Module Recreation Prompt

Use this prompt with the sandboxed AI agent to recreate the Rust module set in `src/agent`.

## Prompt

Recreate a Rust `agent` module for a terminal/TUI coding assistant. The module should be split into these files:

- `mod.rs`
- `provider.rs`
- `worker.rs`
- `cron_scheduler.rs`

The code should model a provider-driven agent runtime with:

- typed request/message structs for sending model input
- a worker thread that accepts commands and emits events
- a provider abstraction with both a trivial echo implementation and a real Codex-backed implementation
- orchestration logic that routes user requests between direct chat, planning, coding, and data-analysis specialists
- a phased specialist loop that advances through `Plan`, `Explore`, `Act`, `Verify`, and `Respond`
- background job support for delegated specialist work
- cron-triggered background specialist execution

Assume the rest of the crate already defines these modules/types and wire to them rather than re-implementing them:

- `crate::api::codex::{CodexClient, ToolCall}`
- `crate::auth::CodexAuth`
- `crate::models::{AgentKind, AgentPhase, BackgroundJob, Board, BoardOperation, ContinuationFrame, ExecutionArtifact, JobStatus, Role, TurnStepStatus}`
- `crate::tools::{self, Sandbox}`
- `crate::tools::cron::pop_due_tasks`

Preserve the behavior and shape below.

## File: `mod.rs`

This file is the public module root.

Requirements:

- Declare `pub mod cron_scheduler;`, `pub mod provider;`, `pub mod worker;`
- Re-export:
  - `pub use provider::Provider;`
  - `pub use worker::WorkerHandles;`
- Define `ProviderAttachment` as:
  - `Image { mime_type: String, data_base64: String }`
  - `File { mime_type: String, filename: String, data_base64: String }`
- Define `ProviderMessage` with:
  - `role: Role`
  - `content: String`
  - `tool_calls: Option<serde_json::Value>`
  - `attachments: Vec<ProviderAttachment>`
- Implement `ProviderMessage::as_json(&self) -> serde_json::Value`
  - For `Role::User`, `Role::System`, and `Role::ToolResult`, serialize text as `{ "type": "input_text", "text": ... }`
  - For `Role::Assistant`, serialize text as `{ "type": "output_text", "text": ... }`
  - Serialize attachments into the same `content` array
  - Map roles:
    - `User -> "user"`
    - `Assistant -> "assistant"`
    - `System` and `ToolResult -> "system"`
- Implement `ProviderAttachment::as_json(&self) -> serde_json::Value`
  - Images become `input_image` with a `data:<mime>;base64,<data>` URL in `image_url`
  - Files become `input_file` with `filename` and `file_data` using the same `data:` URL pattern
- Define `ProviderRequest` with:
  - `messages: Vec<ProviderMessage>`
  - `model: String`
  - `project_id: Option<String>`
  - `board: Option<Board>`
  - `custom_prompt: Option<String>`
  - `agent: AgentKind`
- Define `WorkerCmd` enum:
  - `Send { turn_id, session_id, request }`
  - `UpdateProvider { provider: Box<dyn Provider> }`
  - `Shutdown`
- Define `WorkerEvent` enum with these variants:
  - `Delta { session_id, turn_id, delta }`
  - `Done { session_id, turn_id }`
  - `SystemNote { session_id, turn_id, note }`
  - `ToolStatus { session_id, turn_id, status }`
  - `ToolCalls { session_id, turn_id, calls: serde_json::Value }`
  - `PhaseChange { session_id, turn_id, phase: AgentPhase }`
  - `StepUpdate { session_id, turn_id, phase: AgentPhase, status: TurnStepStatus, summary: Option<String> }`
  - `StepArtifact { session_id, turn_id, phase: AgentPhase, artifact: ExecutionArtifact }`
  - `BoardUpdate { session_id, turn_id, project_id: Option<String>, operations: Vec<crate::models::BoardOperation> }`
  - `JobStarted { session_id, job: BackgroundJob }`
  - `JobUpdated { session_id, job_id, status: crate::models::JobStatus, summary }`
  - `JobMessage { session_id, job_id, content }`
  - `Error { session_id, turn_id, err }`

## File: `worker.rs`

Implement a simple worker runtime using `std::sync::mpsc` plus threads.

Requirements:

- Define `WorkerHandles` with:
  - `cmd_tx: Sender<WorkerCmd>`
  - `event_tx: Sender<WorkerEvent>`
  - `event_rx: Receiver<WorkerEvent>`
  - `provider_label: &'static str`
- Implement `spawn_worker(provider: Arc<dyn Provider>) -> WorkerHandles`
- Behavior:
  - Create command and event channels
  - Store the provider label before moving it
  - Spawn one supervisor thread that loops on `cmd_rx.recv()`
  - On `WorkerCmd::Send`, clone the provider and `event_tx`, then spawn a child thread that calls `provider.generate(&request, session_id, turn_id, &event_tx)`
  - On `UpdateProvider`, replace the current `Arc<dyn Provider>` with `Arc::from(new_provider)`
  - On `Shutdown`, break the loop
  - Return the handles with a cloned `event_tx`

## File: `provider.rs`

This file contains the main orchestration logic.

### Trait and providers

- Define trait `Provider: Debug + Send + Sync + 'static`
  - `fn generate(&self, request: &ProviderRequest, session_id: String, turn_id: String, tx: &Sender<WorkerEvent>);`
  - `fn label(&self) -> &'static str;`
- Implement `EchoProvider`
  - Find the last user message
  - Reply with either:
    - `"(echo provider) empty input."`
    - or a sentence echoing the user text and its char count
  - Stream by splitting on spaces with `split_inclusive(' ')`
  - Sleep about `35ms` per chunk
  - Emit `WorkerEvent::Delta` for each chunk, then `Done`
- Implement `CodexProvider { auth: CodexAuth, client: CodexClient, sandbox: Sandbox }`
  - Constructor `new(auth, workspace: PathBuf) -> anyhow::Result<Self>`
  - Build `CodexClient::new()?`
  - Build `Sandbox::new(workspace)?`
  - Label should be `"orchestrator"`

### `CodexProvider::generate`

Routing rules by `request.agent`:

- `AgentKind::Coder`
  - run `run_specialist_turn(... AgentKind::Coder, "coder", ...)`
  - on error emit `WorkerEvent::Error`
- `AgentKind::Analyst`
  - same pattern with `"analyst"`
- `AgentKind::Planner`
  - call `stream_text_response(..., planner_instructions(request))`
- `AgentKind::Orchestrator`
  - call `route_request(request)` and branch:
    - `Direct` -> `stream_text_response(..., orchestrator_instructions(request))`
    - `Planner` -> `stream_text_response(..., planner_instructions(request))`
    - `Analyst` -> `spawn_background_specialist(... AgentKind::Analyst, "data analysis", ...)`
    - `Coder` -> `spawn_background_specialist(... AgentKind::Coder, "coding", ...)`

### Background specialist delegation

Implement `spawn_background_specialist(...)` that:

- creates a `job_id` via `Uuid::new_v4().to_string()`
- creates and emits `WorkerEvent::JobStarted`
- builds `BackgroundJob` using:
  - `agent`
  - `title: summarize_job_title(request)`
  - `status: JobStatus::Queued`
  - `project_id: request.project_id.clone()`
  - `summary: "queued".to_string()`
- immediately emits a `Delta` to the original turn saying:
  - `I started a {initial_summary} worker in the background. I’ll keep the conversation here and report back when it finishes.`
- clone auth/client/sandbox/request/tx and spawn a thread
- inside the thread:
  - emit `JobUpdated` to `Running` with summary like `"coding..."` or `"data analysis..."`
  - call `run_background_specialist_job(...)`
  - on success emit:
    - `JobUpdated` completed
    - `JobMessage` with returned content
  - on error emit:
    - `JobUpdated` failed with the error string
    - `JobMessage` with prefix `[background {initial_summary} error] ...`
- after spawning, emit `WorkerEvent::Done` for the original turn

### Direct text streaming

Implement `stream_text_response(...) -> Result<()>`:

- Convert `request.messages` with `as_json()`
- Build a streaming Responses API body:
  - `model`
  - `instructions`
  - `input`
  - `tools: []`
  - `tool_choice: "none"`
  - `parallel_tool_calls: false`
  - `reasoning: null`
  - `store: false`
  - `stream: true`
- Call `client.request(auth, &body, session_id.clone(), turn_id.clone(), true, tx)?`
- Emit `Done`

### Specialist execution helpers

Implement:

- `run_specialist_turn(...) -> Result<()>`
  - call `run_specialist_loop(...)`
  - ignore the returned summary
  - emit `Done`
- `run_background_specialist_job(...) -> Result<String>`
  - call `run_specialist_loop(...)`
  - use label mapping:
    - `Coder -> "coder"`
    - `Analyst -> "analyst"`
    - `Orchestrator -> "orchestrator"`
    - `Planner -> "planner"`

### Core phased loop

Implement `pub(crate) fn run_specialist_loop(...) -> Result<String>` with signature compatible with:

- auth/client/sandbox/request
- `agent: AgentKind`
- `label: &str`
- `interactive_turn: Option<(&str, &str, &Sender<WorkerEvent>)>`
- `background_job: Option<(&str, &str, &Sender<WorkerEvent>)>`
- `is_subagent: bool`

Behavior:

- Start with:
  - `conversation = request.messages.clone()`
  - `board = request.board.clone().unwrap_or_default().normalized_for_prompt()`
  - `total_tool_calls = 0`
  - `continuation_frames = Vec<ContinuationFrame>::new()`
  - `final_response = String::new()`
- Iterate across `AgentPhase::ALL`
- At phase start:
  - for interactive turns emit `PhaseChange`, `StepUpdate { Running }`, and `ToolStatus`
  - for background jobs emit `JobUpdated { Running, summary: phase.status() }`
- For each phase, loop up to `board.budgets.max_phase_iterations.max(1)`
- On each iteration:
  - write `BoardOperation::SetLastPhase { phase }`
  - turn `continuation_frames` into extra system messages using `frame.as_system_text()`
  - build `input` from conversation + continuation messages
  - build instructions with `build_specialist_instructions(request, &board, phase, agent, label)`
  - select allowed tools with `tools::tool_specs_for_agent_phase(agent, phase, is_subagent)`
  - `tool_choice = "none"` if empty else `"auto"`
  - only stream assistant text to UI during `Respond` when interactive
  - derive `turn_key` / `session_key` from interactive turn first, otherwise background job
  - call:
    - `client.request(auth, &body, session_key.clone(), turn_key.clone(), emit_text, tx)?`
    - expect `(text_content, tool_calls)`
  - dedupe tool calls with `dedupe_tool_calls`

If tool calls exist:

- For interactive turns emit a `ToolCalls` JSON payload in OpenAI tool-call style:
  - each item has `id`
  - `function.name`
  - `function.arguments`
- For each unique tool call:
  - push `ExecutionArtifact::ToolCall`
  - increment total tool calls
  - fail if `total_tool_calls > board.budgets.max_tool_calls`
  - for interactive turns emit:
    - `SystemNote` using `tools::tool_slug(&call.name, &call.args)`
    - `StepArtifact::ToolCall`
  - construct `tools::SalsaToolContext` with:
    - sandbox/auth/client/session_id/turn_id/tx/model/custom_prompt/is_subagent
  - execute with `tools::execute_tool(&tool_ctx, &call.name, &call.args, board.budgets.max_output_bytes)`
  - apply `execution.board_ops` back onto the board when present
  - append a human-readable summary string like `tool=<name> args=<json> output=<output>`
  - push `ExecutionArtifact::ToolResult`
  - if the tool returned attachments, append a `ProviderMessage` with:
    - `role: Role::ToolResult`
    - text telling the model that the tool attached media from the workspace for inspection and to use the attachment directly
    - those attachments
  - for interactive turns:
    - emit `StepArtifact::BoardOps` and `BoardUpdate` when board ops exist
    - emit `StepArtifact::ToolResult`
- After the tool batch:
  - build a step summary with `compose_step_summary`
  - append a `ContinuationFrame`
  - compact old frames
  - for interactive turns emit `StepUpdate { Completed, summary: Some(step_summary) }`

If no tool calls exist:

- build `step_summary`
- if phase is `Respond`:
  - set `final_response = text_content.clone()`
  - append a final `Role::Assistant` message to conversation
- else if text content is non-empty:
  - for interactive turns emit `StepArtifact::AssistantNote`
  - convert it to a continuation frame and compact
- emit `StepUpdate { Completed, ... }` for interactive turns

Advance phases using `should_advance_phase(...)`.
Return `final_response.trim().to_string()` at the end.

### Instruction builders

Implement:

- `build_specialist_instructions(request, board, phase, agent, label) -> String`
  - Include role-specific text:
    - coder: expert software engineer in bounded phase loop, workspace tools are the only valid way to inspect or modify files
    - analyst: specialized data analysis assistant, must use dataframe tools, must not modify workspace files or fake analysis
    - fallback generic specialist text
  - Always include:
    - never claim to have performed an action without a successful tool call
    - board is authoritative planner state
    - specialist label
    - current phase
    - phase-specific rules
    - serialized board state via `serde_yaml::to_string(board).unwrap_or_default()`
  - Append `request.custom_prompt` if present
- Analyst phase rules:
  - `Plan`: only `board_update`, define analysis goal / target dataset / next question
  - `Explore`: inspect dataset with read-only tools
  - `Act`: run dataframe operations, never write/edit/delete/shell
  - `Verify`: cross-check with follow-up dataframe queries
  - `Respond`: no tools, concise evidence-backed summary with caveats
- Non-analyst phase rules should delegate to `phase.rules()`
- `orchestrator_instructions(request) -> String`
  - conversational primary assistant
  - can clarify intent and help shape plans
  - must not claim to have run tools or edited files itself
  - mentions delegation briefly when appropriate
  - append custom prompt if present
- `planner_instructions(request) -> String`
  - planning specialist
  - flesh out plan when scope is unclear
  - produce concrete steps, risks, and verification ideas
  - concise and actionable
  - must not pretend work already happened
  - append custom prompt if present

### Routing helpers

Create:

- `enum RouteDecision { Direct, Planner, Coder, Analyst }`
- `route_request(request: &ProviderRequest) -> RouteDecision`

Routing behavior:

- Examine the last user message, trimmed and lowercased
- Empty input -> `Direct`
- Very short inputs (<= 3 words, no newline, length < 40) -> `Direct`
- Exact-ish greeting/thanks/chat markers should route `Direct`
- Maintain marker arrays for:
  - direct greetings
  - planner terms
  - analyst terms and phrases
  - coder terms and phrases
- Score markers using substring containment
- Add extra coder score when message has a newline or is long
- Route:
  - planner if planner score > 0 and coder score == 0
  - planner if planner score >= 2 and coder score <= 2
  - analyst if analyst score > coder score and analyst score > 0
  - coder if coder score > 0
  - otherwise direct

Implement helper `marker_score(text, markers) -> u32`.

### Misc helpers

Implement:

- `summarize_job_title(request) -> String`
  - use the first line of the last user message
  - lowercase it
  - strip stopwords like `the`, `a`, `to`, `for`, `this`, `please`, etc.
  - keep up to 5 meaningful words
  - join with `-`
  - fallback `"coding-task"`
- `dedupe_tool_calls(tool_calls: Vec<ToolCall>) -> Vec<ToolCall>`
  - dedupe by `tc.id`
  - allow later calls to replace earlier ones
  - treat null or empty-object args as empty; only keep empties if nothing better exists for that id
- `build_continuation_frame(phase, summary, artifacts) -> ContinuationFrame`
- `compact_continuation_frames(frames)`
  - cap at 6, draining oldest entries
- `compose_step_summary(assistant_text, tool_summaries) -> String`
  - prefer trimmed assistant text
  - else return `"<n> tool actions"`
  - else `"completed"`
- `should_advance_phase(phase, used_tools_this_iteration, iterations, max_iterations) -> bool`
  - stop on max iterations
  - `Plan` and `Respond` always advance after one iteration
  - `Explore`, `Act`, `Verify` continue only while tools were used

## File: `cron_scheduler.rs`

Implement a cron polling thread.

Requirements:

- Function:
  - `pub fn start_cron_scheduler(auth: CodexAuth, client: CodexClient, sandbox: Sandbox, model: String, event_tx: Sender<WorkerEvent>) -> JoinHandle<()>`
- Behavior:
  - spawn a background thread
  - sleep until the next minute boundary using `chrono::Local::now()` and `Timelike::second()`
  - each minute, call `pop_due_tasks()`
  - if no tasks, continue
  - for each due task:
    - clone auth/client/sandbox/model/event sender
    - spawn a child thread
    - create a new `job_id`
    - create a `session_id` like `cron-<first 8 chars of job_id>`
    - emit `WorkerEvent::JobStarted` with a running coder job titled `Cron: <prompt preview>`
      - truncate prompt preview to 40 chars, using an ellipsis after 37 chars
      - set `project_id: None`
      - summary `"running scheduled task..."`
    - build a `ProviderRequest` containing one user message with the task prompt
      - no tool calls
      - no attachments
      - `model`
      - `project_id: None`
      - `board: Some(Board::default())`
      - `custom_prompt: None`
      - `agent: AgentKind::Coder`
    - call `run_specialist_loop(..., AgentKind::Coder, "cron-task", None, Some((&session_id, &job_id, &tx)), true)`
    - on success emit:
      - `JobUpdated` completed with summary `"completed"`
      - `JobMessage` with the returned result
    - on failure emit:
      - `JobUpdated` failed with the error string
      - `JobMessage` containing `[cron error] <err>`

## Implementation notes

- Use `std::thread` and `std::sync::mpsc`, not async runtime code
- Use `serde_json::json!` for request bodies
- Keep all streaming/event emission behavior intact because the UI depends on it
- The phased specialist loop is the core of the design; do not collapse it into a single request/response helper
- The orchestrator should delegate specialist work as background jobs rather than blocking the current conversation when routing to coder/analyst
- Keep the code structured and compile-ready, but rely on the existing crate types for the surrounding system
