use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

use anyhow::{Context, Result};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::agent::{WorkerCmd, WorkerEvent, WorkerHandles, ProviderMessage, ProviderRequest};
use crate::agent::provider::CodexProvider;
use crate::auth::CodexAuth;
use crate::config::{Config, Paths};
use crate::models::{Project, Session, Message, Role, Board};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuAction {
    Settings,
    Sessions,
    Agent,
    Projects,
    Help,
    Quit,
}

impl MenuAction {
    pub const ALL: &'static [(&'static str, MenuAction)] = &[
        ("Settings", MenuAction::Settings),
        ("Sessions", MenuAction::Sessions),
        ("Agent", MenuAction::Agent),
        ("Projects", MenuAction::Projects),
        ("Help", MenuAction::Help),
        ("Quit", MenuAction::Quit),
    ];
}

pub struct App {
    pub(crate) running: bool,
    pub(crate) sessions: Vec<Session>,
    pub(crate) projects: Vec<Project>,
    pub(crate) active_project: Option<usize>,
    pub(crate) next_session_id: u64,
    pub(crate) active_tab: usize,
    pub(crate) user_name: String,
    pub(crate) assistant_name: String,
    pub(crate) default_model: String,
    pub(crate) paths: Paths,
    pub(crate) auth: CodexAuth,
    pub(crate) current_workspace: PathBuf,

    pub(crate) worker_tx: Sender<WorkerCmd>,
    pub(crate) worker_rx: Receiver<WorkerEvent>,
    pub(crate) provider_label: &'static str,
    pub(crate) frame_count: u64,
    pub(crate) tool_status: Option<String>,
    pub(crate) selecting_project: bool,
    pub(crate) selecting_session: bool,

    // Interaction state, refreshed each frame by render_*.
    pub(crate) menu_hits: Vec<(Rect, MenuAction)>,
    pub(crate) tab_hits: Vec<(Rect, usize)>,
    pub(crate) project_hits: Vec<(Rect, usize)>,
    pub(crate) session_hits: Vec<(Rect, usize)>,
    pub(crate) hovered_menu: Option<MenuAction>,
    pub(crate) hovered_project: Option<usize>,
    pub(crate) hovered_session: Option<usize>,
    pub(crate) pressed_menu: Option<MenuAction>,
}

impl App {
    pub fn new(config: Config, paths: Paths, worker: WorkerHandles, auth: CodexAuth) -> Self {
        let mut app = Self {
            running: true,
            sessions: Vec::new(),
            projects: Vec::new(),
            active_project: None,
            next_session_id: 1,
            active_tab: 0,
            user_name: config.user_name,
            assistant_name: config.assistant_name,
            default_model: config.default_model,
            current_workspace: paths.workspace.clone(),
            paths,
            auth,
            worker_tx: worker.cmd_tx,
            worker_rx: worker.event_rx,
            provider_label: worker.provider_label,
            frame_count: 0,
            tool_status: None,
            selecting_project: false,
            selecting_session: false,
            menu_hits: Vec::new(),
            tab_hits: Vec::new(),
            project_hits: Vec::new(),
            session_hits: Vec::new(),
            hovered_menu: None,
            hovered_project: None,
            hovered_session: None,
            pressed_menu: None,
        };

        app.load_sessions().ok();
        app.load_projects().ok();

        if app.sessions.is_empty() {
            app.sessions.push(Session {
                id: 1,
                title: "chat 1".into(),
                messages: Vec::new(),
                input: String::new(),
                pending: false,
                scroll: 0,
            });
            app.next_session_id = 2;
        }

        app
    }

    pub fn load_sessions(&mut self) -> Result<()> {
        let dir = &self.paths.sessions;
        self.sessions.clear();
        self.next_session_id = 1;
        if !dir.exists() { return Ok(()); }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("yaml") {
                let text = fs::read_to_string(&path)?;
                let session: Session = serde_yaml::from_str(&text)?;
                self.next_session_id = self.next_session_id.max(session.id + 1);
                self.sessions.push(session);
            }
        }
        self.sessions.sort_by_key(|s| s.id);
        Ok(())
    }

    pub fn save_active_session(&self) -> Result<()> {
        if let Some(session) = self.sessions.get(self.active_tab) {
            let path = self.paths.sessions.join(format!("{}.yaml", session.title));
            let text = serde_yaml::to_string(session)?;
            fs::write(path, text)?;
        }
        Ok(())
    }

    pub fn load_projects(&mut self) -> Result<()> {
        let dir = &self.paths.projects;
        self.projects.clear();
        if !dir.exists() { return Ok(()); }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let project_file = path.join("project.yaml");
                if project_file.exists() {
                    let text = fs::read_to_string(&project_file)?;
                    let project: Project = serde_yaml::from_str(&text)?;
                    self.projects.push(project);
                }
            }
        }
        Ok(())
    }

    pub fn save_active_project(&mut self) -> Result<()> {
        if let Some(idx) = self.active_project {
            let project = self.projects.get_mut(idx).context("active project not found")?;
            project.sessions = self.sessions.clone();
            project.next_session_id = self.next_session_id;
            let dir = self.paths.projects.join(&project.name);
            fs::create_dir_all(dir.join("workspace"))?;
            let path = dir.join("project.yaml");
            let text = serde_yaml::to_string(project)?;
            fs::write(path, text)?;
        }
        Ok(())
    }

    pub fn switch_project(&mut self, idx: Option<usize>) -> Result<()> {
        if self.active_project.is_some() { self.save_active_project().ok(); }
        else { self.save_active_session().ok(); }

        self.active_project = idx;
        self.sessions.clear();
        self.active_tab = 0;

        let workspace = if let Some(idx) = idx {
            let project = &self.projects[idx];
            self.sessions = project.sessions.clone();
            self.next_session_id = project.next_session_id;
            self.paths.projects.join(&project.name).join("workspace")
        } else {
            self.load_sessions().ok();
            self.paths.workspace.clone()
        };

        self.current_workspace = workspace.clone();
        if self.sessions.is_empty() {
            self.sessions.push(Session { id: 1, title: "chat 1".into(), messages: Vec::new(), input: String::new(), pending: false, scroll: 0 });
            self.next_session_id = 2;
        }

        let provider = Box::new(CodexProvider::new(self.auth.clone(), workspace)?);
        let _ = self.worker_tx.send(WorkerCmd::UpdateProvider { provider });
        Ok(())
    }

    pub fn new_project(&mut self) -> Result<()> {
        let name = format!("project-{}", self.projects.len() + 1);
        self.tool_status = Some(format!("Creating {}...", name));
        let project = Project {
            name: name.clone(),
            sessions: vec![Session { id: 1, title: "chat 1".into(), messages: Vec::new(), input: String::new(), pending: false, scroll: 0 }],
            board: Board::default(),
            next_session_id: 2,
        };
        let dir = self.paths.projects.join(&name);
        fs::create_dir_all(dir.join("workspace"))?;
        let path = dir.join("project.yaml");
        let text = serde_yaml::to_string(&project)?;
        fs::write(path, text)?;
        self.projects.push(project);
        let new_idx = self.projects.len() - 1;
        self.switch_project(Some(new_idx))?;
        self.tool_status = None;
        Ok(())
    }

    pub fn close_session(&mut self, idx: usize) {
        if self.sessions.len() <= 1 { return; }
        self.sessions.remove(idx);
        if self.active_tab >= self.sessions.len() {
            self.active_tab = self.sessions.len().saturating_sub(1);
        }
        if self.active_project.is_some() { self.save_active_project().ok(); }
    }

    pub fn delete_session(&mut self, idx: usize) {
        if self.sessions.len() <= 1 { return; }
        let session = self.sessions.remove(idx);
        if self.active_project.is_none() {
            let path = self.paths.sessions.join(format!("{}.yaml", session.title));
            fs::remove_file(path).ok();
        }
        if self.active_tab >= self.sessions.len() {
            self.active_tab = self.sessions.len().saturating_sub(1);
        }
        if self.active_project.is_some() { self.save_active_project().ok(); }
    }

    pub fn new_session(&mut self) {
        let mut n = self.sessions.len() + 1;
        let mut title = format!("chat {}", n);
        while self.sessions.iter().any(|s| s.title == title) {
            n += 1;
            title = format!("chat {}", n);
        }
        let id = self.next_session_id;
        self.next_session_id += 1;
        self.sessions.push(Session { id, title: title.clone(), messages: Vec::new(), input: String::new(), pending: false, scroll: 0 });
        self.active_tab = self.sessions.len() - 1;
        self.selecting_session = false;
        if self.active_project.is_some() { self.save_active_project().ok(); }
        else { self.save_active_session().ok(); }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press { return; }
        match (key.code, key.modifiers) {
            (KeyCode::Tab, _) => { self.next_tab(); return; }
            (KeyCode::BackTab, _) => { self.prev_tab(); return; }
            _ => {}
        }
        if self.active_session_pending() { return; }
        match (key.code, key.modifiers) {
            (KeyCode::PageUp, _) => { if let Some(s) = self.sessions.get_mut(self.active_tab) { s.scroll = s.scroll.saturating_sub(1); } return; }
            (KeyCode::PageDown, _) => { if let Some(s) = self.sessions.get_mut(self.active_tab) { s.scroll = s.scroll.saturating_add(1); } return; }
            (KeyCode::Enter, _) => self.submit(),
            (KeyCode::Backspace, _) => { if let Some(s) = self.sessions.get_mut(self.active_tab) { s.input.pop(); } }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) => { if let Some(s) = self.sessions.get_mut(self.active_tab) { s.input.push(c); } }
            _ => {}
        }
    }

    pub fn active_session_pending(&self) -> bool { self.sessions.get(self.active_tab).map(|s| s.pending).unwrap_or(false) }

    pub fn submit(&mut self) {
        let Some(session) = self.sessions.get_mut(self.active_tab) else { return; };
        let text = session.input.trim().to_string();
        if text.is_empty() { return; }
        session.input.clear();
        session.messages.push(Message { role: Role::User, body: text });
        session.messages.push(Message { role: Role::Assistant, body: String::new() });
        session.pending = true;
        let session_id = session.id;
        let mut messages: Vec<ProviderMessage> = session.messages.iter().filter(|m| !(matches!(m.role, Role::Assistant) && m.body.is_empty())).map(|m| ProviderMessage { role: m.role, content: m.body.clone() }).collect();
        messages.retain(|m| !m.content.is_empty() || matches!(m.role, Role::Assistant));
        let board = self.active_project.and_then(|idx| self.projects.get(idx).map(|p| serde_json::to_value(&p.board).unwrap()));
        let request = ProviderRequest { messages, model: self.default_model.clone(), board };
        let _ = self.worker_tx.send(WorkerCmd::Send { session_id, request });
        if self.active_project.is_some() { self.save_active_project().ok(); }
        else { self.save_active_session().ok(); }
    }

    pub fn scroll_to_bottom(&mut self) {
        if let Some(session) = self.sessions.get_mut(self.active_tab) {
            let mut lines_len = 0;
            for msg in session.messages.iter() { lines_len += msg.body.lines().count() + 2; }
            let max_scroll = lines_len.saturating_sub(1) as u16;
            session.scroll = max_scroll;
        }
    }

    pub fn drain_worker_events(&mut self) {
        while let Ok(ev) = self.worker_rx.try_recv() {
            match ev {
                WorkerEvent::Delta { session_id, delta } => {
                    if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                        if let Some(last) = s.messages.last_mut() {
                            if matches!(last.role, Role::Assistant) { last.body.push_str(&delta); self.scroll_to_bottom(); }
                        }
                    }
                }
                WorkerEvent::Done { session_id } => {
                    if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) { s.pending = false; self.tool_status = None; self.scroll_to_bottom(); }
                    if self.active_project.is_some() { self.save_active_project().ok(); }
                    else { self.save_active_session().ok(); }
                }
                WorkerEvent::SystemNote { session_id, note } => { if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) { s.messages.push(Message { role: Role::System, body: note }); } }
                WorkerEvent::ToolStatus { session_id, status } => { if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) { if s.pending { self.tool_status = Some(status); } } }
                WorkerEvent::BoardUpdate { board } => { if let Some(idx) = self.active_project { if let Some(project) = self.projects.get_mut(idx) { if let Ok(new_board) = serde_json::from_value(board) { project.board = new_board; } } } }
                WorkerEvent::Error { session_id, err } => {
                    if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                        if let Some(last) = s.messages.last_mut() {
                            if matches!(last.role, Role::Assistant) && last.body.is_empty() { last.body = format!("[error] {}", err); }
                            else { s.messages.push(Message { role: Role::Assistant, body: format!("[error] {}", err) }); }
                        }
                        s.pending = false; self.tool_status = None;
                    }
                    if self.active_project.is_some() { self.save_active_project().ok(); }
                    else { self.save_active_session().ok(); }
                }
            }
        }
    }

    pub fn switch_session(&mut self, idx: usize) { self.active_tab = idx; self.selecting_session = false; self.scroll_to_bottom(); }

    pub fn handle_mouse(&mut self, me: MouseEvent) {
        let pos = Position::new(me.column, me.row);
        match me.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => { self.hovered_menu = self.menu_hit(pos); self.hovered_project = self.project_hit(pos); self.hovered_session = self.session_hit(pos); }
            MouseEventKind::Down(btn) => {
                if btn == MouseButton::Left {
                    if let Some(action) = self.menu_hit(pos) { self.pressed_menu = Some(action); }
                    if let Some(idx) = self.tab_hit(pos) { self.active_tab = idx; }
                    if let Some(idx) = self.project_hit(pos) {
                        if idx == 0 { let _ = self.switch_project(None); }
                        else if idx == self.projects.len() + 1 { let _ = self.new_project(); }
                        else { let _ = self.switch_project(Some(idx - 1)); }
                        self.selecting_project = false;
                    }
                    if self.selecting_session {
                        if let Some(idx) = self.session_hit(pos) {
                            if idx == self.sessions.len() { self.new_session(); }
                            else { self.switch_session(idx); }
                        }
                    }
                } else if btn == MouseButton::Right {
                    if let Some(idx) = self.tab_hit(pos) { self.close_session(idx); }
                    if self.selecting_session {
                        if let Some(idx) = self.session_hit(pos) {
                            if idx < self.sessions.len() { self.delete_session(idx); }
                        }
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => { if let Some(pressed) = self.pressed_menu.take() { if self.menu_hit(pos) == Some(pressed) { self.fire_menu(pressed); } } }
            MouseEventKind::ScrollUp => { if let Some(s) = self.sessions.get_mut(self.active_tab) { s.scroll = s.scroll.saturating_sub(1); } }
            MouseEventKind::ScrollDown => { if let Some(s) = self.sessions.get_mut(self.active_tab) { s.scroll = s.scroll.saturating_add(1); } }
            _ => {}
        }
    }

    pub fn menu_hit(&self, p: Position) -> Option<MenuAction> { self.menu_hits.iter().find(|(r, _)| r.contains(p)).map(|(_, a)| *a) }
    pub fn tab_hit(&self, p: Position) -> Option<usize> { self.tab_hits.iter().find(|(r, _)| r.contains(p)).map(|(_, i)| *i) }
    pub fn project_hit(&self, p: Position) -> Option<usize> { self.project_hits.iter().find(|(r, _)| r.contains(p)).map(|(_, i)| *i) }
    pub fn session_hit(&self, p: Position) -> Option<usize> { self.session_hits.iter().find(|(r, _)| r.contains(p)).map(|(_, i)| *i) }

    pub fn fire_menu(&mut self, action: MenuAction) {
        match action {
            MenuAction::Quit => self.running = false,
            MenuAction::Projects => { self.selecting_project = !self.selecting_project; if self.selecting_project { self.load_projects().ok(); } self.selecting_session = false; }
            MenuAction::Sessions => { self.selecting_session = !self.selecting_session; if self.selecting_session { self.load_sessions().ok(); } self.selecting_project = false; }
            _ => {}
        }
    }

    pub fn next_tab(&mut self) { if !self.sessions.is_empty() { self.active_tab = (self.active_tab + 1) % self.sessions.len(); } }
    pub fn prev_tab(&mut self) { if !self.sessions.is_empty() { let n = self.sessions.len(); self.active_tab = (self.active_tab + n - 1) % n; } }

    pub fn is_running(&self) -> bool { self.running }
}
