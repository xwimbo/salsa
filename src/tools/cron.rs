use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;

use anyhow::{anyhow, Result};
use chrono::{Datelike, Local, Timelike};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::models::BoardOperation;

const MAX_TASKS: usize = 50;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CronTask {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub recurring: bool,
    pub durable: bool,
    pub created_at: u64,
}

static CRON_STORE: Lazy<RwLock<Vec<CronTask>>> = Lazy::new(|| {
    let tasks = load_persistent_tasks();
    RwLock::new(tasks)
});

fn persistent_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".salsa")
        .join("scheduled_tasks.json")
}

fn load_persistent_tasks() -> Vec<CronTask> {
    let path = persistent_path();
    if !path.exists() {
        return Vec::new();
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_persistent_tasks(tasks: &[CronTask]) {
    let durable: Vec<&CronTask> = tasks.iter().filter(|t| t.durable).collect();
    let path = persistent_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, serde_json::to_string_pretty(&durable).unwrap_or_default());
}

pub fn validate_cron(expr: &str) -> bool {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return false;
    }
    for (i, field) in fields.iter().enumerate() {
        let (min, max) = match i {
            0 => (0, 59),   // minute
            1 => (0, 23),   // hour
            2 => (1, 31),   // day of month
            3 => (1, 12),   // month
            4 => (0, 6),    // day of week
            _ => return false,
        };
        if !validate_cron_field(field, min, max) {
            return false;
        }
    }
    true
}

fn validate_cron_field(field: &str, min: u32, max: u32) -> bool {
    if field == "*" {
        return true;
    }
    for part in field.split(',') {
        let part = part.trim();
        if part.contains('/') {
            let pieces: Vec<&str> = part.splitn(2, '/').collect();
            if pieces.len() != 2 {
                return false;
            }
            if pieces[0] != "*" && !validate_cron_range(pieces[0], min, max) {
                return false;
            }
            if pieces[1].parse::<u32>().is_err() {
                return false;
            }
        } else if part.contains('-') {
            if !validate_cron_range(part, min, max) {
                return false;
            }
        } else if let Ok(n) = part.parse::<u32>() {
            if n < min || n > max {
                return false;
            }
        } else {
            return false;
        }
    }
    true
}

fn validate_cron_range(range: &str, min: u32, max: u32) -> bool {
    let parts: Vec<&str> = range.splitn(2, '-').collect();
    if parts.len() != 2 {
        return false;
    }
    let (a, b) = match (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return false,
    };
    a >= min && b <= max && a <= b
}

pub fn cron_matches(expr: &str, now: &chrono::DateTime<Local>) -> bool {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return false;
    }
    let values = [
        now.minute(),
        now.hour(),
        now.day(),
        now.month(),
        now.weekday().num_days_from_sunday(),
    ];
    for (i, field) in fields.iter().enumerate() {
        if !field_matches(field, values[i]) {
            return false;
        }
    }
    true
}

fn field_matches(field: &str, value: u32) -> bool {
    if field == "*" {
        return true;
    }
    for part in field.split(',') {
        let part = part.trim();
        if part.contains('/') {
            let pieces: Vec<&str> = part.splitn(2, '/').collect();
            if let Ok(step) = pieces[1].parse::<u32>() {
                if step == 0 {
                    continue;
                }
                if pieces[0] == "*" {
                    if value % step == 0 {
                        return true;
                    }
                } else if let Some((start, end)) = parse_range(pieces[0]) {
                    if value >= start && value <= end && (value - start) % step == 0 {
                        return true;
                    }
                }
            }
        } else if part.contains('-') {
            if let Some((start, end)) = parse_range(part) {
                if value >= start && value <= end {
                    return true;
                }
            }
        } else if let Ok(n) = part.parse::<u32>() {
            if value == n {
                return true;
            }
        }
    }
    false
}

fn parse_range(s: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = s.splitn(2, '-').collect();
    if parts.len() == 2 {
        if let (Ok(a), Ok(b)) = (parts[0].parse(), parts[1].parse()) {
            return Some((a, b));
        }
    }
    None
}

pub fn pop_due_tasks() -> Vec<CronTask> {
    let now = Local::now();
    let mut store = CRON_STORE.write().unwrap();
    let mut due = Vec::new();
    let mut keep = Vec::new();

    for task in store.drain(..) {
        if cron_matches(&task.cron, &now) {
            due.push(task.clone());
            if task.recurring {
                keep.push(task);
            }
        } else {
            keep.push(task);
        }
    }

    *store = keep;
    save_persistent_tasks(&store);
    due
}

fn cron_to_human(expr: &str) -> String {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return expr.to_string();
    }
    let (min, hour, dom, mon, dow) = (fields[0], fields[1], fields[2], fields[3], fields[4]);

    if expr == "* * * * *" {
        return "every minute".to_string();
    }
    if min != "*" && hour == "*" && dom == "*" && mon == "*" && dow == "*" {
        return format!("every hour at minute {}", min);
    }
    if min != "*" && hour != "*" && dom == "*" && mon == "*" && dow == "*" {
        return format!("daily at {}:{:0>2}", hour, min);
    }
    format!("cron({})", expr)
}

pub fn execute_create(args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let cron_expr = args
        .get("cron")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `cron`"))?;
    let prompt = args
        .get("prompt")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `prompt`"))?;
    let recurring = args
        .get("recurring")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let durable = args
        .get("durable")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if !validate_cron(cron_expr) {
        return Err(anyhow!(
            "invalid cron expression `{}`. Format: \"M H DoM Mon DoW\"",
            cron_expr
        ));
    }

    let mut store = CRON_STORE.write().unwrap();
    if store.len() >= MAX_TASKS {
        return Err(anyhow!("maximum {} scheduled tasks reached", MAX_TASKS));
    }

    let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let task = CronTask {
        id: id.clone(),
        cron: cron_expr.to_string(),
        prompt: prompt.to_string(),
        recurring,
        durable,
        created_at: now,
    };

    store.push(task);
    save_persistent_tasks(&store);

    Ok((
        format!(
            "Scheduled task `{}`: {} — \"{}\"{}",
            id,
            cron_to_human(cron_expr),
            if prompt.len() > 60 {
                format!("{}…", &prompt[..57])
            } else {
                prompt.to_string()
            },
            if recurring { " (recurring)" } else { " (one-shot)" }
        ),
        Vec::new(),
    ))
}

pub fn execute_delete(args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `id`"))?;

    let mut store = CRON_STORE.write().unwrap();
    let before = store.len();
    store.retain(|t| t.id != id);
    let removed = before - store.len();
    save_persistent_tasks(&store);

    if removed > 0 {
        Ok((format!("Deleted scheduled task `{}`.", id), Vec::new()))
    } else {
        Ok((format!("No scheduled task with id `{}`.", id), Vec::new()))
    }
}

pub fn execute_list(_args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let store = CRON_STORE.read().unwrap();
    if store.is_empty() {
        return Ok(("No scheduled tasks.".to_string(), Vec::new()));
    }

    let mut lines = Vec::new();
    for task in store.iter() {
        lines.push(format!(
            "  {} | {} | {}{}",
            task.id,
            cron_to_human(&task.cron),
            if task.prompt.len() > 40 {
                format!("{}…", &task.prompt[..37])
            } else {
                task.prompt.clone()
            },
            if task.recurring {
                " [recurring]"
            } else {
                " [one-shot]"
            }
        ));
    }

    Ok((
        format!("Scheduled tasks ({}):\n{}", store.len(), lines.join("\n")),
        Vec::new(),
    ))
}

pub fn create_spec() -> Value {
    json!({
        "type": "function",
        "name": "cron_create",
        "description": "Create a scheduled task that fires on a cron schedule. Format: \"M H DoM Mon DoW\" (5-field, local time). Use * for wildcards.",
        "parameters": {
            "type": "object",
            "properties": {
                "cron": { "type": "string", "description": "5-field cron expression, e.g. \"0 9 * * 1\" for 9 AM every Monday." },
                "prompt": { "type": "string", "description": "The prompt to execute when the task fires." },
                "recurring": { "type": "boolean", "description": "If true (default), the task recurs. If false, it fires once then deletes." },
                "durable": { "type": "boolean", "description": "If true, the task persists across app restarts." }
            },
            "required": ["cron", "prompt"],
            "additionalProperties": false
        }
    })
}

pub fn delete_spec() -> Value {
    json!({
        "type": "function",
        "name": "cron_delete",
        "description": "Delete a scheduled task by ID.",
        "parameters": {
            "type": "object",
            "properties": {
                "id": { "type": "string" }
            },
            "required": ["id"],
            "additionalProperties": false
        }
    })
}

pub fn list_spec() -> Value {
    json!({
        "type": "function",
        "name": "cron_list",
        "description": "List all scheduled tasks.",
        "parameters": {
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }
    })
}
