use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::provider::run_specialist_loop;
use crate::agent::{ProviderMessage, ProviderRequest, WorkerEvent};
use crate::api::codex::CodexClient;
use crate::auth::CodexAuth;
use crate::models::{
    AgentKind, BackgroundJob, Board, BoardOperation, JobStatus, Role,
};
use crate::tools::Sandbox;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TeamMember {
    name: String,
    role: String,
    prompt: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TeamConfig {
    name: String,
    task: String,
    parallel: bool,
    members: Vec<TeamMember>,
}

fn teams_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".salsa")
        .join("teams")
}

pub fn execute_create(
    auth: &CodexAuth,
    client: &CodexClient,
    sandbox: &Sandbox,
    args: &Value,
    session_id: &str,
    _turn_id: &str,
    tx: &Sender<WorkerEvent>,
    model: &str,
    custom_prompt: Option<&str>,
) -> Result<(String, Vec<BoardOperation>)> {
    let team_name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `name`"))?;
    let task = args
        .get("task")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `task`"))?;
    let parallel = args
        .get("parallel")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let members_raw = args
        .get("members")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("missing `members` array"))?;

    if members_raw.is_empty() {
        return Err(anyhow!("team must have at least one member"));
    }
    if members_raw.len() > 8 {
        return Err(anyhow!("maximum 8 team members"));
    }

    let mut members = Vec::new();
    for m in members_raw {
        let name = m
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("team member missing `name`"))?;
        let role = m
            .get("role")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("team member missing `role`"))?;
        let prompt = m.get("prompt").and_then(|v| v.as_str()).map(String::from);
        members.push(TeamMember {
            name: name.to_string(),
            role: role.to_string(),
            prompt,
        });
    }

    let config = TeamConfig {
        name: team_name.to_string(),
        task: task.to_string(),
        parallel,
        members: members.clone(),
    };

    // Persist config
    let team_dir = teams_dir().join(team_name);
    let _ = fs::create_dir_all(&team_dir);
    let _ = fs::write(
        team_dir.join("config.json"),
        serde_json::to_string_pretty(&config).unwrap_or_default(),
    );

    // Report team creation
    let job_id = Uuid::new_v4().to_string();
    let _ = tx.send(WorkerEvent::JobStarted {
        session_id: session_id.to_string(),
        job: BackgroundJob {
            id: job_id.clone(),
            agent: AgentKind::Orchestrator,
            title: format!("Team: {}", team_name),
            status: JobStatus::Running,
            project_id: None,
            summary: format!("{} members, {}", members.len(), if parallel { "parallel" } else { "sequential" }),
        },
    });

    // Run agents
    let mut results: Vec<(String, String)> = Vec::new();

    if parallel {
        let handles: Vec<_> = members
            .iter()
            .map(|member| {
                let auth = auth.clone();
                let client = client.clone();
                let sandbox = sandbox.clone();
                let model = model.to_string();
                let custom_prompt = custom_prompt.map(String::from);
                let member_name = member.name.clone();
                let member_role = member.role.clone();
                let member_prompt = member.prompt.clone();
                let task = task.to_string();

                thread::spawn(move || {
                    let prompt_text = member_prompt.unwrap_or_else(|| {
                        format!(
                            "You are {} with the role: {}.\n\nTeam task: {}",
                            member_name, member_role, task
                        )
                    });

                    let request = ProviderRequest {
                        messages: vec![ProviderMessage {
                            role: Role::User,
                            content: prompt_text,
                            tool_calls: None,
                            attachments: Vec::new(),
                        }],
                        model,
                        project_id: None,
                        board: Some(Board::default()),
                        custom_prompt: custom_prompt,
                        agent: AgentKind::Coder,
                    };

                    let result = run_specialist_loop(
                        &auth,
                        &client,
                        &sandbox,
                        &request,
                        AgentKind::Coder,
                        &member_name,
                        None,
                        None,
                        true, // is_subagent
                    );

                    (
                        member_name,
                        result.unwrap_or_else(|e| format!("[error] {}", e)),
                    )
                })
            })
            .collect();

        for handle in handles {
            if let Ok(result) = handle.join() {
                results.push(result);
            }
        }
    } else {
        for member in &members {
            let prompt_text = member.prompt.clone().unwrap_or_else(|| {
                format!(
                    "You are {} with the role: {}.\n\nTeam task: {}",
                    member.name, member.role, task
                )
            });

            let request = ProviderRequest {
                messages: vec![ProviderMessage {
                    role: Role::User,
                    content: prompt_text,
                    tool_calls: None,
                    attachments: Vec::new(),
                }],
                model: model.to_string(),
                project_id: None,
                board: Some(Board::default()),
                custom_prompt: custom_prompt.map(String::from),
                agent: AgentKind::Coder,
            };

            let result = run_specialist_loop(
                auth,
                client,
                sandbox,
                &request,
                AgentKind::Coder,
                &member.name,
                None,
                None,
                true,
            );

            results.push((
                member.name.clone(),
                result.unwrap_or_else(|e| format!("[error] {}", e)),
            ));
        }
    }

    // Save results
    let _ = fs::write(
        team_dir.join("results.json"),
        serde_json::to_string_pretty(&results).unwrap_or_default(),
    );

    let _ = tx.send(WorkerEvent::JobUpdated {
        session_id: session_id.to_string(),
        job_id,
        status: JobStatus::Completed,
        summary: format!("{} members completed", results.len()),
    });

    // Build output
    let mut output = format!("Team `{}` completed.\n\n", team_name);
    for (name, result) in &results {
        output.push_str(&format!("--- {} ---\n{}\n\n", name, result));
    }

    Ok((output, Vec::new()))
}

pub fn execute_delete(args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `name`"))?;

    let team_dir = teams_dir().join(name);
    if !team_dir.exists() {
        return Ok((format!("Team `{}` not found.", name), Vec::new()));
    }

    fs::remove_dir_all(&team_dir)
        .map_err(|e| anyhow!("failed to delete team `{}`: {}", name, e))?;

    Ok((format!("Team `{}` deleted.", name), Vec::new()))
}

pub fn create_spec() -> Value {
    json!({
        "type": "function",
        "name": "team_create",
        "description": "Create a team of agents to work on a task together. Each member gets their own sub-agent loop. Members can run in parallel or sequentially.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Team name (used for persistence)." },
                "task": { "type": "string", "description": "The shared task description for the team." },
                "parallel": { "type": "boolean", "description": "If true (default), members run in parallel. If false, sequentially." },
                "members": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "Member name/identifier." },
                            "role": { "type": "string", "description": "The member's role/specialization." },
                            "prompt": { "type": "string", "description": "Optional custom prompt. If omitted, a default prompt with name+role+task is used." }
                        },
                        "required": ["name", "role"]
                    },
                    "description": "List of team members (max 8)."
                }
            },
            "required": ["name", "task", "members"],
            "additionalProperties": false
        }
    })
}

pub fn delete_spec() -> Value {
    json!({
        "type": "function",
        "name": "team_delete",
        "description": "Delete a team and its persisted config/results.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"],
            "additionalProperties": false
        }
    })
}
