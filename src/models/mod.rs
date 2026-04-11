use serde::{Deserialize, Serialize};
use serde_json;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Role {
    User,
    Assistant,
    System,
    ToolResult,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub body: String,
    #[serde(default)]
    pub tool_calls: Option<serde_json::Value>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: u64,
    pub title: String,
    pub messages: Vec<Message>,
    #[serde(skip)]
    pub input: String,
    #[serde(skip)]
    pub pending: bool,
    #[serde(skip)]
    pub scroll: u16,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Board {
    pub vision: String,
    pub steps: Vec<String>,
    pub completed_steps: Vec<String>,
    pub issues: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub sessions: Vec<Session>,
    pub board: Board,
    pub next_session_id: u64,
}
