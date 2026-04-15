use std::fs;
use std::path::Path;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use super::sandbox::Sandbox;
use crate::models::BoardOperation;

fn strip_frontmatter(content: &str) -> &str {
    if !content.starts_with("---") {
        return content;
    }
    // Find closing ---
    if let Some(end) = content[3..].find("\n---") {
        let after = 3 + end + 4; // skip past "\n---"
        if after < content.len() {
            return content[after..].trim_start_matches('\n');
        }
    }
    content
}

fn find_skills(search_dirs: &[&Path]) -> Vec<(String, String)> {
    let mut skills = Vec::new();
    for dir in search_dirs {
        if !dir.exists() {
            continue;
        }
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    let name = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    if !skills.iter().any(|(n, _)| *n == name) {
                        skills.push((name, path.display().to_string()));
                    }
                }
            }
        }
    }
    skills.sort_by(|a, b| a.0.cmp(&b.0));
    skills
}

pub fn execute(sandbox: &Sandbox, args: &Value) -> Result<(String, Vec<BoardOperation>)> {
    let skill = args
        .get("skill")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `skill`"))?;
    let arguments = args.get("args").and_then(|v| v.as_str()).unwrap_or("");

    let home_commands = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".salsa")
        .join("commands");
    let workspace_commands = sandbox.resolve(".salsa/commands").unwrap_or_default();

    let search_dirs: Vec<&Path> = vec![workspace_commands.as_path(), home_commands.as_path()];

    if skill == "list" {
        let skills = find_skills(&search_dirs);
        if skills.is_empty() {
            return Ok((
                "No skills found. Place .md files in ~/.salsa/commands/ or <workspace>/.salsa/commands/".to_string(),
                Vec::new(),
            ));
        }
        let listing: Vec<String> = skills
            .iter()
            .map(|(name, path)| format!("  {} ({})", name, path))
            .collect();
        return Ok((
            format!("Available skills ({}):\n{}", skills.len(), listing.join("\n")),
            Vec::new(),
        ));
    }

    // Find the skill file
    let mut skill_path = None;
    for dir in &search_dirs {
        let candidate = dir.join(format!("{}.md", skill));
        if candidate.exists() {
            skill_path = Some(candidate);
            break;
        }
    }

    let path = skill_path.ok_or_else(|| {
        anyhow!(
            "skill `{}` not found. Search paths: {:?}",
            skill,
            search_dirs
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
        )
    })?;

    let raw = fs::read_to_string(&path)
        .map_err(|e| anyhow!("failed to read skill `{}`: {}", skill, e))?;

    let content = strip_frontmatter(&raw);
    let expanded = content.replace("$ARGUMENTS", arguments);

    Ok((expanded, Vec::new()))
}

pub fn spec() -> Value {
    json!({
        "type": "function",
        "name": "skill",
        "description": "Load and execute a user-defined skill from ~/.salsa/commands/ or <workspace>/.salsa/commands/. Skills are markdown files with optional YAML frontmatter. Use skill=\"list\" to see available skills.",
        "parameters": {
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "Name of the skill to execute, or \"list\" to enumerate available skills."
                },
                "args": {
                    "type": "string",
                    "description": "Optional arguments that replace $ARGUMENTS in the skill template."
                }
            },
            "required": ["skill"],
            "additionalProperties": false
        }
    })
}
