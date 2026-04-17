# AGENTS.md

## Repository snapshot
- Rust application with a single crate (`Cargo.toml`) and no test directory or CI config checked in.
- Main runtime is a ratatui/crossterm TUI that boots from `src/main.rs` and persists state under `~/.salsa/`.
- There is a product-vision file in `CLAUDE.md`; it is worth reading before making UX or architecture decisions because it explains the intended TUI feel, sandbox model, storage layout, and default model choice.

## Commands that actually exist
- Build: `cargo build`
- Run the app: `cargo run`
- Release build: `cargo build --release`
- Standard verification fallback: `cargo test`

Notes:
- I did not find a `Makefile`, `justfile`, `Taskfile`, or GitHub Actions workflow, so prefer plain Cargo commands.
- The README shows `cargo build --release` followed by `./target/release/salsa`.

## High-level architecture
### Entry point and runtime loop
- `src/main.rs` sets up raw terminal mode, alternate screen, mouse capture, and a panic hook that restores the terminal on crash.
- The app redraw loop targets ~60 FPS via `FRAME_BUDGET = 16_667µs` and drains worker events before every frame.
- Config/bootstrap happens first via `config::bootstrap()`, then auth is loaded from `~/.codex/auth.json`; if auth loading fails, the app falls back to `EchoProvider` instead of the real Codex-backed provider.

### Main layers
- `src/app/mod.rs`: stateful application controller for sessions, projects, overlays, input handling, persistence, and worker event integration.
- `src/ui/mod.rs` and `src/ui/theme.rs`: rendering only. Hit-testing rectangles are rebuilt during render and consumed by mouse handling in `App`.
- `src/agent/provider.rs`: provider routing/orchestration logic, phased agent loop, specialist routing, background job spawning.
- `src/agent/worker.rs`: background worker thread that receives `WorkerCmd` and emits `WorkerEvent`.
- `src/tools/*.rs`: tool implementations and tool schemas exposed to the Codex backend.
- `src/models/mod.rs`: serialized domain types for sessions, projects, boards, tasks, jobs, execution artifacts, and phases.
- `src/config/mod.rs`: `~/.salsa` path resolution and config bootstrap.
- `src/api/codex.rs`: blocking SSE client for the ChatGPT Codex responses endpoint.
- `src/auth/mod.rs`: loads Codex auth from disk only; there is no in-app login flow here.

## Control flow that matters
### User message path
1. `App::submit()` appends the user message and a placeholder assistant message, marks the session pending, and sends `WorkerCmd::Send`.
2. `agent::worker::spawn_worker()` spawns a thread per request and calls `Provider::generate`.
3. `CodexProvider::generate()` either answers directly, routes to planner, or starts a coder/analyst specialist, sometimes as a background job.
4. Specialist work runs through `run_specialist_loop()` in `src/agent/provider.rs`.
5. The loop iterates through `AgentPhase::Plan/Explore/Act/Verify/Respond`, asks the model for tool calls, executes tools, updates the board, and emits `WorkerEvent`s.
6. `App::drain_worker_events()` is the only place UI state is updated from worker output.

### Project/session persistence
- Global sessions live under `~/.salsa/sessions/*.yaml`.
- Project sessions live under `~/.salsa/projects/<project-id>/sessions/*.yaml`.
- Project metadata is stored in `~/.salsa/projects/<project-id>/project.yaml`.
- `App::save_active_session()` writes a compacted snapshot, not the in-memory session verbatim.
- `App::compacted_session_snapshot()` intentionally strips pending state, turn steps, tool results, and empty assistant messages before persistence. If you are debugging “missing” session details on disk, this is why.

## Non-obvious conventions and gotchas
### Workspace sandboxing is strict and path-relative
- File tools are rooted in `Sandbox`, defined in `src/tools/sandbox.rs`.
- `Sandbox::resolve()` rejects absolute paths and any `..` traversal, and checks canonicalized ancestors to prevent symlink escapes.
- Tool-facing file paths are expected to be relative to the current workspace root, not repo-absolute.

### Project switching changes the live provider sandbox
- `App::switch_project()` updates `current_workspace` and then sends `WorkerCmd::UpdateProvider` with a newly constructed `CodexProvider` rooted at that workspace.
- If behavior seems inconsistent between global/project scopes, inspect `current_workspace` and whether provider replacement happened.

### Sessions shown in the UI are not raw history
- Tool results and internal system state are intentionally hidden from the chat UI (`Role::System` and `Role::ToolResult` are suppressed in `render_chat`).
- The visible assistant tool indicator is only shown while the session is pending and only on the last assistant message.

### The phased agent loop is board-driven
- `Board` in `src/models/mod.rs` is not optional bookkeeping; prompts include the serialized board state every phase.
- `Board::normalized_for_prompt()` silently applies default budgets when unset: 2 phase iterations, 8 tool calls, and 12_000 output bytes.
- `run_specialist_loop()` enforces tool budgets and advances Explore/Act/Verify when an iteration completes without tool use.

### Tool availability depends on phase and agent kind
- Tool specs come from `tools::tool_specs_for_agent_phase()`.
- `Respond` has no tools.
- Analyst agents do not get write/edit/delete/shell tools.
- Sub-agents are explicitly blocked from spawning more sub-agents or teams.

### Background tasks are in-memory in some places
- Worker/background jobs shown in the jobs pane are persisted through sessions.
- `src/tools/tasks.rs` uses a global `DashMap` task store (`TASK_STORE`) with no filesystem persistence, so those tool-managed tasks are process-local.

### Web fetch has home-directory side effects
- `src/tools/web.rs` caches fetched content under `~/.salsa/web_cache/` for 15 minutes, truncates output to 100_000 chars, and strips HTML with a custom parser.
- If you change fetch behavior, verify both cache semantics and HTML-to-text output.

### Codex API client is blocking SSE
- `src/api/codex.rs` uses `reqwest::blocking` and manually parses SSE frames.
- The endpoint is hardcoded to `https://chatgpt.com/backend-api/codex/responses` with `OpenAI-Beta: responses=experimental`.
- Tool calls are discovered by recursively scanning the whole SSE JSON payload, not just one field.

### UI hit-testing depends on render order
- `menu_hits`, `tab_hits`, `project_hits`, etc. are recomputed while rendering and then used by mouse handlers.
- If you add or move interactive widgets, update both rendering and the associated hit-test vectors together.

## Source layout details worth knowing
### `src/app/mod.rs`
- Owns almost all mutable app state.
- Handles slash commands (`/new`, `/new project`, `/del session`, `/del project`, `/help`, `/add`) directly in `submit()` before invoking the provider.
- Manages onboarding/profile editing and file-browser overlays.
- Copies selected external files into the current workspace via `file_browser_confirm()`.

### `src/agent/provider.rs`
- `route_request()` is simple heuristic routing based on message text.
- Orchestrator may answer directly or delegate to planner/coder/analyst.
- Background specialist work emits a quick conversational acknowledgment, then marks the user-facing turn done while the job continues.
- Continuation across phases is summarized via `ContinuationFrame` instead of replaying raw tool outputs back to the model.

### `src/tools/mod.rs`
- Central registry for tool execution and JSON schemas.
- `execute_fs_edit()` requires `old_text` to match exactly once; it errors on zero or multiple matches.
- `execute_sh_run()` shells out with workspace root as current directory.

### `src/ui/mod.rs`
- Menu bar shows current project/global scope, provider label, and workspace path in the top status text.
- Jobs pane appears only when enabled, terminal width is at least 80, and the active session has jobs.
- Pending input state replaces the input box with tool/phase progress instead of editable text.

## Style and implementation patterns observed
- Single-file `mod.rs` modules are used heavily rather than many nested files.
- State mutation is centralized in `App`; rendering methods mostly compute visual state from that mutable app struct.
- Serialization types derive `Serialize`/`Deserialize` directly in `src/models/mod.rs`.
- Error handling uses `anyhow` broadly with `Context` on filesystem/network boundaries.
- JSON tool schemas are handwritten with `serde_json::json!`, not generated.
- The codebase currently uses blocking IO and threads rather than async runtime patterns.

## Testing and validation
- I did not find any checked-in Rust unit tests or integration tests.
- For changes, the safest validation path is Cargo-based:
  - `cargo build` for compile validation
  - `cargo test` for regression checks if/when tests exist
  - `cargo run` for manual TUI verification when behavior is UI-specific
- Because this is a terminal UI, many important behaviors are only realistically verified by running the app and exercising keyboard/mouse flows.

## Agent guidance from existing rule/context files
From `CLAUDE.md`:
- The intended product is a polished Rust TUI assistant inspired by Claude Code/Codex CLI/OpenClaw.
- The UX target is a smooth ratatui app with mouse support, GitHub Dark styling, and minimal container titles.
- The default model should be `gpt-5.3-codex`.
- Workspaces are split between a global `~/.salsa/workspace` and per-project workspaces under each project directory.
- If a tool touches or creates something inside the workspace, it is expected to auto-run.

## Practical advice for future agents
- Read `CLAUDE.md` before changing UX, storage layout, or provider behavior; it contains product decisions not obvious from the code.
- When debugging “why didn’t this persist?”, check whether the data lives in session YAML, project YAML, `~/.salsa/config.yaml`, cache files, or only in memory.
- When changing tool behavior, inspect both the tool schema and the executor implementation; they are manually kept in sync.
- When changing routed agent behavior, inspect `route_request()`, `tool_specs_for_agent_phase()`, and `AgentPhase::rules()` together.
- Be careful when editing session persistence code: the compaction behavior is intentional and affects what the app can restore after restart.
- For UI interactions, verify both keyboard and mouse paths; the app supports both and often has separate logic for each.
