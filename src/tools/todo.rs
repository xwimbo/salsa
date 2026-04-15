use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::models::BoardOperation;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
    #[serde(default)]
    pub priority: Option<String>,
}

fn todos_path(session_id: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".salsa")
        .join("todos")
        .join(format!("{}.json", session_id))
}

fn load_todos(session_id: &str) -> Vec<TodoItem> {
    let path = todos_path(session_id);
    if !path.exists() {
        return Vec::new();
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_todos(session_id: &str, todos: &[TodoItem]) {
    let path = todos_path(session_id);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, serde_json::to_string_pretty(todos).unwrap_or_default());
}

fn validate_transition(old: &TodoStatus, new: &TodoStatus) -> bool {
    matches!(
        (old, new),
        (TodoStatus::Pending, TodoStatus::InProgress)
            | (TodoStatus::Pending, TodoStatus::Completed)
            | (TodoStatus::InProgress, TodoStatus::Completed)
    )
}

pub fn execute(session_id: &str, args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let items = args
        .get("todos")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("missing `todos` array"))?;

    let mut new_todos: Vec<TodoItem> = Vec::new();
    for item in items {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("todo item missing `id`"))?;
        let content = item
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("todo item missing `content`"))?;
        let status_str = item
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending");
        let status = match status_str {
            "pending" => TodoStatus::Pending,
            "in_progress" => TodoStatus::InProgress,
            "completed" => TodoStatus::Completed,
            other => return Err(anyhow!("invalid status `{}`", other)),
        };
        let priority = item.get("priority").and_then(|v| v.as_str()).map(String::from);

        // Check for duplicate IDs in input
        if new_todos.iter().any(|t| t.id == id) {
            return Err(anyhow!("duplicate id `{}` in input", id));
        }

        new_todos.push(TodoItem {
            id: id.to_string(),
            content: content.to_string(),
            status,
            priority,
        });
    }

    // Validate transitions against persisted state
    let existing = load_todos(session_id);
    let mut warnings = Vec::new();
    for new_item in &new_todos {
        if let Some(old_item) = existing.iter().find(|o| o.id == new_item.id) {
            if old_item.status != new_item.status
                && !validate_transition(&old_item.status, &new_item.status)
            {
                warnings.push(format!(
                    "invalid transition for `{}`: {:?} → {:?} (ignored, kept {:?})",
                    new_item.id, old_item.status, new_item.status, old_item.status
                ));
            }
        }
    }

    // Apply valid transitions
    let mut final_todos = new_todos.clone();
    for item in &mut final_todos {
        if let Some(old_item) = existing.iter().find(|o| o.id == item.id) {
            if old_item.status != item.status
                && !validate_transition(&old_item.status, &item.status)
            {
                item.status = old_item.status.clone();
            }
        }
    }

    save_todos(session_id, &final_todos);

    let pending = final_todos
        .iter()
        .filter(|t| t.status == TodoStatus::Pending)
        .count();
    let in_progress = final_todos
        .iter()
        .filter(|t| t.status == TodoStatus::InProgress)
        .count();
    let completed = final_todos
        .iter()
        .filter(|t| t.status == TodoStatus::Completed)
        .count();

    let mut output = format!(
        "Updated {} todos: {} pending, {} in_progress, {} completed.",
        final_todos.len(),
        pending,
        in_progress,
        completed
    );

    if !warnings.is_empty() {
        output.push_str("\nWarnings:\n");
        for w in &warnings {
            output.push_str("  - ");
            output.push_str(w);
            output.push('\n');
        }
    }

    Ok((output, Vec::new()))
}

pub fn spec() -> Value {
    json!({
        "type": "function",
        "name": "todo_write",
        "description": "Write a structured todo list for the current session. Supports status transitions: pending → in_progress → completed. Backwards transitions are rejected.",
        "parameters": {
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string", "description": "Unique identifier for this todo item." },
                            "content": { "type": "string", "description": "Description of the task." },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "Current status of the task."
                            },
                            "priority": { "type": "string", "description": "Optional priority level." }
                        },
                        "required": ["id", "content", "status"]
                    }
                }
            },
            "required": ["todos"],
            "additionalProperties": false
        }
    })
}
