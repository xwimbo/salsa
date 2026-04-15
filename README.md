# Salsa

A terminal-based AI assistant built in Rust with [ratatui](https://ratatui.rs). Salsa provides a full TUI experience with project management, session tabs, a phased agent loop, and a rich set of workspace tools -- all powered by the Codex API.

## What It Does

Salsa is an AI coding assistant that runs entirely in your terminal. You type a message, and the agent plans, explores your workspace, makes changes, verifies its work, and responds -- all within a sandboxed directory. It cannot see or touch files outside its workspace.

The agent operates in a five-phase loop inspired by how tools like Claude Code and Codex CLI handle tasks:

1. **Plan** -- Understand the goal, break it into tasks on the project board.
2. **Explore** -- Read files, list directories, fetch web pages, gather context.
3. **Act** -- Write code, edit files, run shell commands, spawn sub-agents.
4. **Verify** -- Re-read files, run tests, confirm the work is correct.
5. **Respond** -- Summarize what was done in plain language.

Each phase has access to only the tools it needs. The agent cannot claim it edited a file without actually calling the tool.

## Features

- **60 FPS TUI** with full mouse support, scrollable chat, and color-coded output (GitHub Dark theme).
- **Session tabs** -- Multiple conversations open at once, each with its own history. `Ctrl+N` to create, `Tab`/`Shift+Tab` to switch.
- **Projects** -- Isolated workspaces with their own sessions, prompts, and boards. Create and switch from the Projects menu.
- **Phased agent loop** -- Plan/Explore/Act/Verify/Respond with per-phase tool restrictions and a project board for state management.
- **Background jobs** -- Long-running agent tasks appear in a sidebar and don't block the chat.
- **Sub-agents** -- The agent can spawn child agents for parallel work. Sub-agents are sandboxed and cannot spawn further sub-agents.
- **Teams** -- Coordinate up to 8 agents working on a shared task, in parallel or sequentially.
- **Cron scheduler** -- Schedule recurring tasks with standard 5-field cron expressions. Tasks persist across restarts.
- **Web fetch** -- Fetch and cache web pages with automatic HTML stripping (15-minute cache TTL).
- **Skills** -- User-defined command templates as Markdown files. Place `.md` files in `~/.salsa/commands/` and invoke them by name.
- **Custom prompts** -- Global or per-project system prompts, editable from the Prompt menu.
- **Data analysis** -- Built-in Polars-powered dataframe tools for inspecting CSVs, Parquet files, and computing statistics.

## Installation

Requires Rust 1.75+.

```bash
git clone <repo-url>
cd salsa
cargo build --release
./target/release/salsa
```

## Authentication

Salsa reads the OAuth token that [Codex CLI](https://github.com/openai/codex) stores on disk. If you've already authenticated with Codex CLI, Salsa will pick up the token automatically. If no token is found, Salsa falls back to an echo provider for testing the UI.

## Directory Structure

Everything lives under `~/.salsa/`:

```
~/.salsa/
  config.yaml          # User settings (name, model, prompt, API key)
  workspace/           # Default sandbox for file operations
  projects/            # Per-project directories
    <project>/
      workspace/       # Project-specific sandbox
      sessions/        # Project sessions (YAML)
      project.yaml     # Project metadata + board
  sessions/            # Global sessions (YAML)
  agents/              # Agent personality files
  commands/            # User-defined skills (Markdown)
  todos/               # Per-session todo lists (JSON)
  teams/               # Team configs and results
  web_cache/           # Cached web fetches
  scheduled_tasks.json # Persistent cron tasks
```

## Usage

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `Ctrl+N` | New session |
| `Tab` / `Shift+Tab` | Next / previous tab |
| `Ctrl+R` | Rename current session |
| `Enter` | Send message |
| `PageUp` / `PageDown` | Scroll chat |
| `Esc` | Close overlay / cancel |

### Menu Bar

Click or use the menu items at the top:

- **Settings** -- Toggle the jobs pane, edit your profile.
- **Prompt** -- View and edit the system prompt (global or per-project).
- **Projects** -- Switch between projects or create a new one.
- **Help** -- Show keyboard shortcuts and feature reference.
- **Quit** -- Exit the application.

### Tools Available to the Agent

The agent has access to these tools depending on which phase it's in:

| Tool | Description | Phases |
|------|-------------|--------|
| `fs_read` | Read a file | Explore, Act, Verify |
| `fs_list` | List a directory | Explore, Act, Verify |
| `fs_write` | Create/overwrite a file | Act |
| `fs_edit` | Search-and-replace in a file | Act |
| `fs_delete` | Delete a file | Act |
| `sh_run` | Run a shell command | Act, Verify |
| `web_fetch` | Fetch a URL | Explore, Act, Verify |
| `agent` | Spawn a sub-agent | Explore, Act |
| `team_create` | Create a multi-agent team | Explore, Act |
| `skill` | Run a user-defined skill | Explore, Act |
| `brief` | Send a formatted message | All |
| `board_update` | Update the project board | All |
| `cron_create/delete/list` | Manage scheduled tasks | Act |
| `task_create/update/list` | Track work items | All |
| `todo_write` | Per-session todo list | Act |
| `enter/exit_plan_mode` | Toggle read-only mode | All |

### Skills

Place Markdown files in `~/.salsa/commands/` to define reusable skills:

```markdown
---
name: review
description: Code review helper
---

Review the following code for bugs, style issues, and potential improvements.
Focus on: $ARGUMENTS
```

The agent can invoke this with `skill("review", args="the auth module")`. The `$ARGUMENTS` placeholder is replaced with the provided arguments.

### Cron Tasks

The agent can schedule tasks using standard cron syntax:

- `0 9 * * 1` -- 9 AM every Monday
- `*/5 * * * *` -- Every 5 minutes
- `0 0 1 * *` -- Midnight on the 1st of each month

Scheduled tasks run as sub-agent loops in the background. Mark a task as `durable: true` to persist it across restarts.

## Configuration

Edit `~/.salsa/config.yaml`:

```yaml
user_name: <USERNAME>
assistant_name: assistant
default_model: gpt-5.3-codex
global_prompt: "You are a helpful assistant..."
show_jobs_pane: true
```

## License

MIT
