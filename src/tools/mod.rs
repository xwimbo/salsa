pub mod sandbox;

use std::fs;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
pub use sandbox::Sandbox;
use serde_json::{json, Value};

pub fn tool_specs() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "name": "fs_read",
            "description": "Read the full contents of a text file inside the sandboxed workspace.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the workspace root."}
                },
                "required": ["path"],
                "additionalProperties": false
            }
        }),
        json!({
            "type": "function",
            "name": "fs_list",
            "description": "List the entries in a directory inside the sandboxed workspace. Use '.' for the workspace root.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory path relative to the workspace root."}
                },
                "required": ["path"],
                "additionalProperties": false
            }
        }),
        json!({
            "type": "function",
            "name": "fs_write",
            "description": "Create or overwrite a text file in the sandboxed workspace. Parent directories are created as needed.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }
        }),
        json!({
            "type": "function",
            "name": "fs_edit",
            "description": "Replace the single exact occurrence of old_text with new_text in an existing file. Fails if old_text is missing or appears more than once; include enough surrounding context to make it unique.",
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
        }),
        json!({
            "type": "function",
            "name": "fs_delete",
            "description": "Delete a file in the sandboxed workspace.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"],
                "additionalProperties": false
            }
        }),
        json!({
            "type": "function",
            "name": "sh_run",
            "description": "Execute a shell command in the sandboxed workspace. Returns stdout and stderr combined.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {"type": "string"}
                },
                "required": ["command"],
                "additionalProperties": false
            }
        }),
        json!({
            "type": "function",
            "name": "board_update",
            "description": "Update the project board with the latest vision, steps, and issues.",
            "parameters": {
                "type": "object",
                "properties": {
                    "vision": {"type": "string", "description": "The overall vision for the project."},
                    "steps": {"type": "array", "items": {"type": "string"}, "description": "The remaining steps to complete the vision."},
                    "completed_steps": {"type": "array", "items": {"type": "string"}, "description": "The steps that have been completed."},
                    "issues": {"type": "array", "items": {"type": "string"}, "description": "Any current issues or blockers."}
                },
                "additionalProperties": false
            }
        }),
    ]
}

pub fn tool_slug(name: &str, args: &Value) -> String {
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
        other => format!("Using {} on {}", other, path),
    }
}

pub fn execute_tool(sandbox: &Sandbox, name: &str, args: &Value) -> Result<String> {
    match name {
        "fs_read" => {
            let path = string_arg(args, "path")?;
            let abs = sandbox.resolve(path)?;
            let content = fs::read_to_string(&abs)
                .with_context(|| format!("reading {}", path))?;
            Ok(content)
        }
        "fs_list" => {
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
            if entries.is_empty() {
                Ok(format!("(empty directory: {})", path))
            } else {
                Ok(entries.join("\n"))
            }
        }
        "fs_write" => {
            let path = string_arg(args, "path")?;
            let content = string_arg(args, "content")?;
            let abs = sandbox.resolve(path)?;
            if let Some(parent) = abs.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating parents for {}", path))?;
            }
            fs::write(&abs, content).with_context(|| format!("writing {}", path))?;
            Ok(format!("wrote {} bytes to {}", content.len(), path))
        }
        "fs_edit" => {
            let path = string_arg(args, "path")?;
            let old_text = string_arg(args, "old_text")?;
            let new_text = string_arg(args, "new_text")?;
            let abs = sandbox.resolve(path)?;
            let current = fs::read_to_string(&abs)
                .with_context(|| format!("reading {}", path))?;
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
            Ok(format!("edited {}", path))
        }
        "fs_delete" => {
            let path = string_arg(args, "path")?;
            let abs = sandbox.resolve(path)?;
            fs::remove_file(&abs).with_context(|| format!("deleting {}", path))?;
            Ok(format!("deleted {}", path))
        }
        "sh_run" => {
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
            Ok(format!("{}{}", stdout, stderr))
        }
        "board_update" => Ok("Board updated successfully.".to_string()),
        _ => bail!("unknown tool: {}", name),
    }
}

fn string_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing or non-string arg `{}`", key))
}
