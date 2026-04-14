use std::fs;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde_json::{json, Value};

use crate::agent::ProviderAttachment;
use crate::models::BoardOperation;
use crate::tools::Sandbox;

pub struct ToolExecution {
    pub output: String,
    pub board_ops: Vec<BoardOperation>,
    pub attachments: Vec<ProviderAttachment>,
}

pub fn tool_specs() -> Vec<Value> {
    vec![view_image_spec(), view_pdf_spec()]
}

pub fn execute(sandbox: &Sandbox, name: &str, args: &Value) -> Result<ToolExecution> {
    match name {
        "view_image" => execute_view_image(sandbox, args),
        "view_pdf" => execute_view_pdf(sandbox, args),
        _ => bail!("unknown media tool: {}", name),
    }
}

pub fn tool_slug(name: &str, args: &Value) -> Option<String> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
    match name {
        "view_image" => Some(format!("Attaching image {} for model inspection", path)),
        "view_pdf" => Some(format!("Attaching PDF {} for model inspection", path)),
        _ => None,
    }
}

fn execute_view_image(sandbox: &Sandbox, args: &Value) -> Result<ToolExecution> {
    let path = string_arg(args, "path")?;
    let abs = sandbox.resolve(path)?;
    let bytes = fs::read(&abs).with_context(|| format!("reading {}", path))?;
    let mime_type = infer_image_mime_type(path)?;
    let data_base64 = base64::engine::general_purpose::STANDARD.encode(bytes);

    Ok(ToolExecution {
        output: format!("attached image {} ({})", path, mime_type),
        board_ops: Vec::new(),
        attachments: vec![ProviderAttachment::Image {
            mime_type: mime_type.to_string(),
            data_base64,
        }],
    })
}

fn execute_view_pdf(sandbox: &Sandbox, args: &Value) -> Result<ToolExecution> {
    let path = string_arg(args, "path")?;
    let abs = sandbox.resolve(path)?;
    let bytes = fs::read(&abs).with_context(|| format!("reading {}", path))?;
    let data_base64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let filename = abs
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("invalid pdf filename"))?
        .to_string();

    Ok(ToolExecution {
        output: format!("attached pdf {}", path),
        board_ops: Vec::new(),
        attachments: vec![ProviderAttachment::File {
            mime_type: "application/pdf".to_string(),
            filename,
            data_base64,
        }],
    })
}

fn string_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing or non-string arg `{}`", key))
}

fn infer_image_mime_type(path: &str) -> Result<&'static str> {
    let ext = path
        .rsplit('.')
        .next()
        .map(|value| value.to_ascii_lowercase())
        .ok_or_else(|| anyhow!("image path must include an extension"))?;

    match ext.as_str() {
        "png" => Ok("image/png"),
        "jpg" | "jpeg" => Ok("image/jpeg"),
        "webp" => Ok("image/webp"),
        "gif" => Ok("image/gif"),
        "bmp" => Ok("image/bmp"),
        other => bail!("unsupported image format `{}`", other),
    }
}

fn view_image_spec() -> Value {
    json!({
        "type": "function",
        "name": "view_image",
        "description": "Attach a local workspace image so the model can inspect it visually. Use this for screenshots, diagrams, photos, charts, and other images.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Image path relative to the workspace root."}
            },
            "required": ["path"],
            "additionalProperties": false
        }
    })
}

fn view_pdf_spec() -> Value {
    json!({
        "type": "function",
        "name": "view_pdf",
        "description": "Attach a local workspace PDF so the model can inspect its contents. Use this when the task depends on reading or reviewing a PDF.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "PDF path relative to the workspace root."}
            },
            "required": ["path"],
            "additionalProperties": false
        }
    })
}
