use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::Utc;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::models::BoardOperation;

pub static TASK_STORE: Lazy<Arc<DashMap<String, Task>>> =
    Lazy::new(|| Arc::new(DashMap::new()));

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Running,
    Failed,
    Deleted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub blocks: Vec<String>,
    #[serde(default)]
    pub blocked_by: Vec<String>,
    #[serde(default)]
    pub metadata: Option<Value>,
    #[serde(default)]
    pub output: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub fn execute_create(args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let subject = args
        .get("subject")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `subject`"))?;
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let task = Task {
        id: id.clone(),
        subject: subject.to_string(),
        description: description.to_string(),
        status: TaskStatus::Pending,
        owner: args.get("owner").and_then(|v| v.as_str()).map(String::from),
        blocks: Vec::new(),
        blocked_by: Vec::new(),
        metadata: args.get("metadata").cloned(),
        output: None,
        created_at: now.clone(),
        updated_at: now,
    };

    TASK_STORE.insert(id.clone(), task);
    Ok((format!("Task created: {}", id), Vec::new()))
}

pub fn execute_get(args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let id = args
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `task_id`"))?;

    match TASK_STORE.get(id) {
        Some(task) => {
            let json = serde_json::to_string_pretty(task.value())
                .unwrap_or_else(|_| format!("{:?}", task.value()));
            Ok((json, Vec::new()))
        }
        None => Ok((format!("Task `{}` not found.", id), Vec::new())),
    }
}

pub fn execute_update(args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let id = args
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `task_id`"))?;

    let mut task = TASK_STORE
        .get_mut(id)
        .ok_or_else(|| anyhow!("task `{}` not found", id))?;

    if let Some(subject) = args.get("subject").and_then(|v| v.as_str()) {
        task.subject = subject.to_string();
    }
    if let Some(desc) = args.get("description").and_then(|v| v.as_str()) {
        task.description = desc.to_string();
    }
    if let Some(status_str) = args.get("status").and_then(|v| v.as_str()) {
        let new_status = match status_str {
            "pending" => TaskStatus::Pending,
            "in_progress" => TaskStatus::InProgress,
            "completed" => TaskStatus::Completed,
            "running" => TaskStatus::Running,
            "failed" => TaskStatus::Failed,
            "deleted" => TaskStatus::Deleted,
            other => return Err(anyhow!("invalid status `{}`", other)),
        };
        task.status = new_status;
    }
    if let Some(owner) = args.get("owner").and_then(|v| v.as_str()) {
        task.owner = Some(owner.to_string());
    }
    if let Some(meta) = args.get("metadata") {
        task.metadata = Some(meta.clone());
    }
    if let Some(output) = args.get("output").and_then(|v| v.as_str()) {
        task.output = Some(output.to_string());
    }
    if let Some(blocks) = args.get("blocks").and_then(|v| v.as_array()) {
        for b in blocks {
            if let Some(bid) = b.as_str() {
                if !task.blocks.contains(&bid.to_string()) {
                    task.blocks.push(bid.to_string());
                }
            }
        }
    }
    if let Some(blocked_by) = args.get("blocked_by").and_then(|v| v.as_array()) {
        for b in blocked_by {
            if let Some(bid) = b.as_str() {
                if !task.blocked_by.contains(&bid.to_string()) {
                    task.blocked_by.push(bid.to_string());
                }
            }
        }
    }

    task.updated_at = Utc::now().to_rfc3339();
    Ok((format!("Task `{}` updated.", id), Vec::new()))
}

pub fn execute_list(args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let include_all = args
        .get("include_completed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut items: Vec<Value> = Vec::new();
    for entry in TASK_STORE.iter() {
        let task = entry.value();
        if !include_all
            && matches!(task.status, TaskStatus::Completed | TaskStatus::Deleted)
        {
            continue;
        }
        items.push(json!({
            "id": task.id,
            "subject": task.subject,
            "status": task.status,
            "owner": task.owner,
            "blocked_by": task.blocked_by,
        }));
    }

    if items.is_empty() {
        return Ok(("No active tasks.".to_string(), Vec::new()));
    }

    let output = serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string());
    Ok((output, Vec::new()))
}

pub fn execute_stop(args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let id = args
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `task_id`"))?;

    let mut task = TASK_STORE
        .get_mut(id)
        .ok_or_else(|| anyhow!("task `{}` not found", id))?;

    if matches!(task.status, TaskStatus::Running | TaskStatus::InProgress) {
        task.status = TaskStatus::Failed;
        task.updated_at = Utc::now().to_rfc3339();
        Ok((format!("Task `{}` stopped.", id), Vec::new()))
    } else {
        Ok((
            format!(
                "Task `{}` is {:?}, cannot stop.",
                id, task.status
            ),
            Vec::new(),
        ))
    }
}

pub fn execute_output(args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let id = args
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `task_id`"))?;
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `text`"))?;

    let mut task = TASK_STORE
        .get_mut(id)
        .ok_or_else(|| anyhow!("task `{}` not found", id))?;

    let current = task.output.clone().unwrap_or_default();
    task.output = Some(format!("{}{}\n", current, text));
    task.updated_at = Utc::now().to_rfc3339();
    Ok((format!("Output appended to task `{}`.", id), Vec::new()))
}

pub fn create_spec() -> Value {
    json!({
        "type": "function",
        "name": "task_create",
        "description": "Create a new task to track work progress.",
        "parameters": {
            "type": "object",
            "properties": {
                "subject": { "type": "string", "description": "Brief title for the task." },
                "description": { "type": "string", "description": "Detailed description." },
                "owner": { "type": "string", "description": "Optional owner name." },
                "metadata": { "type": "object", "description": "Optional metadata." }
            },
            "required": ["subject"],
            "additionalProperties": false
        }
    })
}

pub fn get_spec() -> Value {
    json!({
        "type": "function",
        "name": "task_get",
        "description": "Get full details of a task by ID.",
        "parameters": {
            "type": "object",
            "properties": {
                "task_id": { "type": "string" }
            },
            "required": ["task_id"],
            "additionalProperties": false
        }
    })
}

pub fn update_spec() -> Value {
    json!({
        "type": "function",
        "name": "task_update",
        "description": "Update a task's subject, description, status, owner, metadata, output, blocks, or blocked_by.",
        "parameters": {
            "type": "object",
            "properties": {
                "task_id": { "type": "string" },
                "subject": { "type": "string" },
                "description": { "type": "string" },
                "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "running", "failed", "deleted"] },
                "owner": { "type": "string" },
                "metadata": { "type": "object" },
                "output": { "type": "string" },
                "blocks": { "type": "array", "items": { "type": "string" } },
                "blocked_by": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["task_id"],
            "additionalProperties": false
        }
    })
}

pub fn list_spec() -> Value {
    json!({
        "type": "function",
        "name": "task_list",
        "description": "List all active tasks. Set include_completed=true to include completed/deleted tasks.",
        "parameters": {
            "type": "object",
            "properties": {
                "include_completed": { "type": "boolean" }
            },
            "additionalProperties": false
        }
    })
}

pub fn stop_spec() -> Value {
    json!({
        "type": "function",
        "name": "task_stop",
        "description": "Stop a running or in-progress task (marks it as failed).",
        "parameters": {
            "type": "object",
            "properties": {
                "task_id": { "type": "string" }
            },
            "required": ["task_id"],
            "additionalProperties": false
        }
    })
}

pub fn output_spec() -> Value {
    json!({
        "type": "function",
        "name": "task_output",
        "description": "Append text output to a task.",
        "parameters": {
            "type": "object",
            "properties": {
                "task_id": { "type": "string" },
                "text": { "type": "string" }
            },
            "required": ["task_id", "text"],
            "additionalProperties": false
        }
    })
}
