use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

pub struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)
            .with_context(|| format!("creating sandbox root {}", root.display()))?;
        let canon = fs::canonicalize(&root)
            .with_context(|| format!("canonicalizing sandbox root {}", root.display()))?;
        Ok(Self { root: canon })
    }

    /// Resolve a workspace-relative path. Refuses absolute paths, `..`
    /// traversal, and symlink escapes.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf> {
        let p = Path::new(rel);
        if p.is_absolute() {
            bail!("path must be relative to the workspace");
        }
        for comp in p.components() {
            match comp {
                Component::ParentDir => bail!("path cannot contain '..'"),
                Component::Prefix(_) | Component::RootDir => bail!("invalid path"),
                _ => {}
            }
        }
        let joined = self.root.join(p);

        // Canonicalize either the target or its nearest existing ancestor to
        // defeat symlink-based escapes.
        let check_base = if joined.exists() {
            fs::canonicalize(&joined)
                .with_context(|| format!("canonicalizing {}", joined.display()))?
        } else {
            let mut cursor: &Path = joined.as_path();
            let existing = loop {
                match cursor.parent() {
                    Some(parent) if parent.exists() => break parent,
                    Some(parent) => cursor = parent,
                    None => bail!("path has no existing ancestor"),
                }
            };
            fs::canonicalize(existing)
                .with_context(|| format!("canonicalizing ancestor {}", existing.display()))?
        };

        if !check_base.starts_with(&self.root) {
            bail!("path escapes workspace");
        }
        Ok(joined)
    }
}

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
    ]
}

pub fn tool_slug(name: &str, args: &Value) -> String {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
    match name {
        "fs_read" => format!("Using readTool to read file {}", path),
        "fs_list" => format!("Using listTool to list directory {}", path),
        "fs_write" => format!("Using writeTool to write file {}", path),
        "fs_edit" => format!("Using editTool to edit file {}", path),
        "fs_delete" => format!("Using deleteTool to delete file {}", path),
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
        _ => bail!("unknown tool: {}", name),
    }
}

fn string_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing or non-string arg `{}`", key))
}
