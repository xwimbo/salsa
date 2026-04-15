pub mod agent;
pub mod analysis;
pub mod brief;
pub mod cron;
pub mod media;
pub mod plan_mode;
pub mod sandbox;
pub mod skills;
pub mod tasks;
pub mod team;
pub mod todo;
pub mod web;

use std::fs;
use std::process::Command;
use std::sync::mpsc::Sender;

use anyhow::{anyhow, bail, Context, Result};
pub use sandbox::Sandbox;
use serde_json::{json, Value};

use crate::agent::WorkerEvent;
use crate::api::codex::CodexClient;
use crate::auth::CodexAuth;
use crate::models::{AgentKind, AgentPhase, BoardOperation};

/// Bundles all context needed by tools that require auth, networking, or sub-agent spawning.
pub struct SalsaToolContext<'a> {
    pub sandbox: &'a Sandbox,
    pub auth: &'a CodexAuth,
    pub client: &'a CodexClient,
    pub session_id: &'a str,
    pub turn_id: &'a str,
    pub tx: &'a Sender<WorkerEvent>,
    pub model: &'a str,
    pub custom_prompt: Option<&'a str>,
    pub is_subagent: bool,
}

pub fn tool_specs_for_agent_phase(agent: AgentKind, phase: AgentPhase, is_subagent: bool) -> Vec<Value> {
    match agent {
        AgentKind::Analyst => analyst_tool_specs_for_phase(phase),
        AgentKind::Orchestrator | AgentKind::Planner | AgentKind::Coder => {
            default_tool_specs_for_phase(phase, is_subagent)
        }
    }
}

pub fn tool_slug(name: &str, args: &Value) -> String {
    if let Some(slug) = analysis::tool_slug(name, args) {
        return slug;
    }
    if let Some(slug) = media::tool_slug(name, args) {
        return slug;
    }
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
    let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("?");
    match name {
        "fs_read" => format!("Using readTool to read file {}", path),
        "fs_list" => format!("Using listTool to list directory {}", path),
        "fs_write" => format!("Using writeTool to write file {}", path),
        "fs_edit" => format!("Using editTool to edit file {}", path),
        "fs_delete" => format!("Using deleteTool to delete file {}", path),
        "sh_run" => format!("Using shellTool to run command: {}", command),
        "board_update" => "Updating project board...".to_string(),
        "brief" => "Sending brief...".to_string(),
        "enter_plan_mode" => "Entering plan mode...".to_string(),
        "exit_plan_mode" => "Exiting plan mode...".to_string(),
        "web_fetch" => {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("?");
            format!("Fetching {}...", url)
        }
        "todo_write" => "Updating todos...".to_string(),
        "task_create" => "Creating task...".to_string(),
        "task_get" | "task_list" => "Reading tasks...".to_string(),
        "task_update" | "task_stop" | "task_output" => "Updating task...".to_string(),
        "cron_create" => "Scheduling task...".to_string(),
        "cron_delete" => "Removing scheduled task...".to_string(),
        "cron_list" => "Listing scheduled tasks...".to_string(),
        "skill" => {
            let skill_name = args.get("skill").and_then(|v| v.as_str()).unwrap_or("?");
            format!("Running skill: {}", skill_name)
        }
        "agent" => {
            let desc = args.get("description").and_then(|v| v.as_str()).unwrap_or("sub-agent");
            format!("Spawning agent: {}", desc)
        }
        "team_create" => {
            let team = args.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            format!("Creating team: {}", team)
        }
        "team_delete" => "Deleting team...".to_string(),
        other => format!("Using {} on {}", other, path),
    }
}

pub fn execute_tool(
    ctx: &SalsaToolContext,
    name: &str,
    args: &Value,
    max_output_bytes: usize,
) -> media::ToolExecution {
    let result = match name {
        // Filesystem tools
        "fs_read" => execute_fs_read(ctx.sandbox, args).map(tool_output_only),
        "fs_list" => execute_fs_list(ctx.sandbox, args).map(tool_output_only),
        "fs_write" => execute_fs_write(ctx.sandbox, args).map(tool_output_only),
        "fs_edit" => execute_fs_edit(ctx.sandbox, args).map(tool_output_only),
        "fs_delete" => execute_fs_delete(ctx.sandbox, args).map(tool_output_only),
        "sh_run" => execute_sh_run(ctx.sandbox, args).map(tool_output_only),
        "board_update" => execute_board_update(args).map(tool_output_only),

        // Analysis tools
        "df_inspect" | "df_describe" | "df_filter" | "df_group_stats" | "df_value_counts"
        | "df_correlation" => analysis::execute(ctx.sandbox, name, args).map(|output| media::ToolExecution {
            output,
            board_ops: Vec::new(),
            attachments: Vec::new(),
        }),

        // Media tools
        "view_image" | "view_pdf" => media::execute(ctx.sandbox, name, args),

        // Brief
        "brief" => brief::execute(ctx.sandbox, args).map(tool_output_only),

        // Plan mode
        "enter_plan_mode" => plan_mode::execute_enter(args).map(tool_output_only),
        "exit_plan_mode" => plan_mode::execute_exit(args).map(tool_output_only),

        // Web fetch
        "web_fetch" => web::execute(args).map(tool_output_only),

        // Todo
        "todo_write" => todo::execute(ctx.session_id, args).map(tool_output_only),

        // Tasks
        "task_create" => tasks::execute_create(args).map(tool_output_only),
        "task_get" => tasks::execute_get(args).map(tool_output_only),
        "task_update" => tasks::execute_update(args).map(tool_output_only),
        "task_list" => tasks::execute_list(args).map(tool_output_only),
        "task_stop" => tasks::execute_stop(args).map(tool_output_only),
        "task_output" => tasks::execute_output(args).map(tool_output_only),

        // Cron
        "cron_create" => cron::execute_create(args).map(tool_output_only),
        "cron_delete" => cron::execute_delete(args).map(tool_output_only),
        "cron_list" => cron::execute_list(args).map(tool_output_only),

        // Skills
        "skill" => skills::execute(ctx.sandbox, args).map(tool_output_only),

        // Agent (sub-agent spawning — blocked for sub-agents to prevent recursion)
        "agent" => {
            if ctx.is_subagent {
                Err(anyhow!("sub-agents cannot spawn further sub-agents"))
            } else {
                agent::execute(
                    ctx.auth, ctx.client, ctx.sandbox, args,
                    ctx.session_id, ctx.turn_id, ctx.tx,
                    ctx.model, ctx.custom_prompt,
                ).map(tool_output_only)
            }
        }

        // Team (blocked for sub-agents)
        "team_create" => {
            if ctx.is_subagent {
                Err(anyhow!("sub-agents cannot create teams"))
            } else {
                team::execute_create(
                    ctx.auth, ctx.client, ctx.sandbox, args,
                    ctx.session_id, ctx.turn_id, ctx.tx,
                    ctx.model, ctx.custom_prompt,
                ).map(tool_output_only)
            }
        }
        "team_delete" => team::execute_delete(args).map(tool_output_only),

        _ => Err(anyhow!("unknown tool: {}", name)),
    };

    match result {
        Ok(mut execution) => {
            execution.output = truncate_output(execution.output, max_output_bytes);
            execution
        }
        Err(err) => media::ToolExecution {
            output: format!("[error] {}", err),
            board_ops: Vec::new(),
            attachments: Vec::new(),
        },
    }
}

fn default_tool_specs_for_phase(phase: AgentPhase, is_subagent: bool) -> Vec<Value> {
    // Tools available in every non-Respond phase
    let always = || vec![
        brief::spec(),
        plan_mode::enter_spec(),
        plan_mode::exit_spec(),
        tasks::create_spec(),
        tasks::get_spec(),
        tasks::update_spec(),
        tasks::list_spec(),
        tasks::stop_spec(),
        tasks::output_spec(),
    ];

    match phase {
        AgentPhase::Plan => {
            let mut specs = vec![board_update_spec()];
            specs.extend(always());
            specs
        }
        AgentPhase::Explore => {
            let mut specs = vec![fs_read_spec(), fs_list_spec(), board_update_spec()];
            specs.extend(media::tool_specs());
            specs.extend(always());
            specs.push(web::spec());
            specs.push(skills::spec());
            if !is_subagent {
                specs.push(agent::spec());
                specs.push(team::create_spec());
                specs.push(team::delete_spec());
            }
            specs
        }
        AgentPhase::Act => {
            let mut specs = vec![
                fs_read_spec(),
                fs_list_spec(),
                fs_write_spec(),
                fs_edit_spec(),
                fs_delete_spec(),
                sh_run_spec(),
                board_update_spec(),
            ];
            specs.extend(media::tool_specs());
            specs.extend(always());
            specs.push(web::spec());
            specs.push(todo::spec());
            specs.push(skills::spec());
            specs.push(cron::create_spec());
            specs.push(cron::delete_spec());
            specs.push(cron::list_spec());
            if !is_subagent {
                specs.push(agent::spec());
                specs.push(team::create_spec());
                specs.push(team::delete_spec());
            }
            specs
        }
        AgentPhase::Verify => {
            let mut specs = vec![fs_read_spec(), fs_list_spec(), sh_run_spec(), board_update_spec()];
            specs.extend(media::tool_specs());
            specs.extend(always());
            specs.push(web::spec());
            specs.push(cron::list_spec());
            specs
        }
        AgentPhase::Respond => Vec::new(),
    }
}

fn analyst_tool_specs_for_phase(phase: AgentPhase) -> Vec<Value> {
    match phase {
        AgentPhase::Plan => vec![board_update_spec()],
        AgentPhase::Explore | AgentPhase::Act | AgentPhase::Verify => {
            let mut specs = vec![fs_read_spec(), fs_list_spec(), board_update_spec()];
            specs.extend(analysis::tool_specs());
            specs.extend(media::tool_specs());
            specs
        }
        AgentPhase::Respond => Vec::new(),
    }
}

fn execute_fs_read(sandbox: &Sandbox, args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let path = string_arg(args, "path")?;
    let abs = sandbox.resolve(path)?;
    let content = fs::read_to_string(&abs).with_context(|| format!("reading {}", path))?;
    Ok((content, Vec::new()))
}

fn execute_fs_list(sandbox: &Sandbox, args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let path = string_arg(args, "path")?;
    let abs = sandbox.resolve(path)?;
    let mut entries: Vec<String> = fs::read_dir(&abs)
        .with_context(|| format!("listing {}", path))?
        .filter_map(|e| e.ok())
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let suffix = match e.file_type() {
                Ok(ft) if ft.is_dir() => "/",
                _ => "",
            };
            format!("{}{}", name, suffix)
        })
        .collect();
    entries.sort();
    let output = if entries.is_empty() {
        format!("(empty directory: {})", path)
    } else {
        entries.join("\n")
    };
    Ok((output, Vec::new()))
}

fn execute_fs_write(sandbox: &Sandbox, args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let path = string_arg(args, "path")?;
    let content = string_arg(args, "content")?;
    let abs = sandbox.resolve(path)?;
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating parents for {}", path))?;
    }
    fs::write(&abs, content).with_context(|| format!("writing {}", path))?;
    Ok((
        format!("wrote {} bytes to {}", content.len(), path),
        Vec::new(),
    ))
}

fn execute_fs_edit(sandbox: &Sandbox, args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let path = string_arg(args, "path")?;
    let old_text = string_arg(args, "old_text")?;
    let new_text = string_arg(args, "new_text")?;
    let abs = sandbox.resolve(path)?;
    let current = fs::read_to_string(&abs).with_context(|| format!("reading {}", path))?;
    let count = current.matches(old_text).count();
    if count == 0 {
        bail!("old_text not found in {}", path);
    }
    if count > 1 {
        bail!(
            "old_text is not unique in {} ({} occurrences); include more surrounding context",
            path,
            count
        );
    }
    let updated = current.replacen(old_text, new_text, 1);
    fs::write(&abs, &updated).with_context(|| format!("writing {}", path))?;
    Ok((format!("edited {}", path), Vec::new()))
}

fn execute_fs_delete(sandbox: &Sandbox, args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let path = string_arg(args, "path")?;
    let abs = sandbox.resolve(path)?;
    fs::remove_file(&abs).with_context(|| format!("deleting {}", path))?;
    Ok((format!("deleted {}", path), Vec::new()))
}

fn execute_sh_run(sandbox: &Sandbox, args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let command = string_arg(args, "command")?;
    let output = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", command])
            .current_dir(&sandbox.root)
            .output()?
    } else {
        Command::new("sh")
            .args(["-c", command])
            .current_dir(&sandbox.root)
            .output()?
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok((format!("{}{}", stdout, stderr), Vec::new()))
}

fn execute_board_update(args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let ops_value = args
        .get("operations")
        .cloned()
        .ok_or_else(|| anyhow!("missing operations"))?;
    let operations: Vec<BoardOperation> =
        serde_json::from_value(ops_value).context("parsing board_update operations")?;
    Ok((
        format!("applied {} board operations", operations.len()),
        operations,
    ))
}

fn truncate_output(mut output: String, max_output_bytes: usize) -> String {
    if max_output_bytes == 0 || output.len() <= max_output_bytes {
        return output;
    }

    output.truncate(max_output_bytes);
    output.push_str("\n[truncated]");
    output
}

fn tool_output_only((output, board_ops): (String, Vec<BoardOperation>)) -> media::ToolExecution {
    media::ToolExecution {
        output,
        board_ops,
        attachments: Vec::new(),
    }
}

fn string_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing or non-string arg `{}`", key))
}

fn fs_read_spec() -> Value {
    json!({
        "type": "function",
        "name": "fs_read",
        "description": "Read the full contents of a text file inside the workspace root.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path relative to the workspace root."}
            },
            "required": ["path"],
            "additionalProperties": false
        }
    })
}

fn fs_list_spec() -> Value {
    json!({
        "type": "function",
        "name": "fs_list",
        "description": "List the entries in a directory inside the workspace root. Use '.' for the workspace root.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Directory path relative to the workspace root."}
            },
            "required": ["path"],
            "additionalProperties": false
        }
    })
}

fn fs_write_spec() -> Value {
    json!({
        "type": "function",
        "name": "fs_write",
        "description": "Create or overwrite a text file in the workspace root. Parent directories are created as needed.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"}
            },
            "required": ["path", "content"],
            "additionalProperties": false
        }
    })
}

fn fs_edit_spec() -> Value {
    json!({
        "type": "function",
        "name": "fs_edit",
        "description": "Replace a single exact occurrence of old_text with new_text in an existing file.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "old_text": {"type": "string"},
                "new_text": {"type": "string"}
            },
            "required": ["path", "old_text", "new_text"],
            "additionalProperties": false
        }
    })
}

fn fs_delete_spec() -> Value {
    json!({
        "type": "function",
        "name": "fs_delete",
        "description": "Delete a file in the workspace root.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {"type": "string"}
            },
            "required": ["path"],
            "additionalProperties": false
        }
    })
}

fn sh_run_spec() -> Value {
    json!({
        "type": "function",
        "name": "sh_run",
        "description": "Execute a shell command with the workspace as current directory. Use only when file tools are insufficient.",
        "parameters": {
            "type": "object",
            "properties": {
                "command": {"type": "string"}
            },
            "required": ["command"],
            "additionalProperties": false
        }
    })
}

fn board_update_spec() -> Value {
    json!({
        "type": "function",
        "name": "board_update",
        "description": "Apply reducer-style updates to the project board. Use this to plan, select tasks, record facts, record attempts, and mark outcomes.",
        "parameters": {
            "type": "object",
            "properties": {
                "operations": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "op": {
                                "type": "string",
                                "enum": [
                                    "set_goal",
                                    "set_summary",
                                    "set_current_task",
                                    "add_task",
                                    "update_task_status",
                                    "record_attempt",
                                    "add_task_evidence",
                                    "add_fact",
                                    "add_blocker",
                                    "clear_blockers",
                                    "set_budget",
                                    "set_last_phase"
                                ]
                            },
                            "goal": {"type": "string"},
                            "summary": {"type": "string"},
                            "task_id": {"type": ["string", "null"]},
                            "id": {"type": "string"},
                            "title": {"type": "string"},
                            "deps": {"type": "array", "items": {"type": "string"}},
                            "acceptance_criteria": {"type": "array", "items": {"type": "string"}},
                            "status": {
                                "type": "string",
                                "enum": ["todo", "in_progress", "done", "blocked"]
                            },
                            "last_error": {"type": ["string", "null"]},
                            "evidence": {"type": "string"},
                            "fact": {"type": "string"},
                            "blocker": {"type": "string"},
                            "max_phase_iterations": {"type": "integer"},
                            "max_tool_calls": {"type": "integer"},
                            "max_output_bytes": {"type": "integer"},
                            "phase": {
                                "type": "string",
                                "enum": ["plan", "explore", "act", "verify", "respond"]
                            }
                        },
                        "required": ["op"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["operations"],
            "additionalProperties": false
        }
    })
}
