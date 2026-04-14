use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::agent::{WorkerCmd, WorkerEvent, WorkerHandles, ProviderMessage, ProviderRequest};
use crate::agent::provider::CodexProvider;
use crate::auth::CodexAuth;
use crate::config::{Config, Paths};
use crate::models::{
    AgentKind, AgentPhase, Board, ExecutionArtifact, JobStatus, Message, Project, Role, Session,
    TurnStep, TurnStepStatus,
};
use serde_yaml;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuAction {
    Settings,
    Prompt,
    Projects,
    Help,
    Quit,
}

impl MenuAction {
    pub const ALL: &'static [(&'static str, MenuAction)] = &[
        ("Settings", MenuAction::Settings),
        ("Prompt", MenuAction::Prompt),
        ("Projects", MenuAction::Projects),
        ("Help", MenuAction::Help),
        ("Quit", MenuAction::Quit),
    ];
}

pub struct App {
    pub(crate) running: bool,
    pub(crate) sessions: Vec<Session>,
    pub(crate) projects: Vec<Project>,
    pub(crate) active_project_id: Option<String>,
    pub(crate) active_session_id: Option<String>,
    pub(crate) active_tab: usize,
    pub(crate) user_name: String,
    pub(crate) assistant_name: String,
    pub(crate) default_model: String,
    pub(crate) global_prompt: String,
    pub(crate) show_jobs_pane: bool,
    pub(crate) onboarding_complete: bool,
    pub(crate) first_name: String,
    pub(crate) last_name: String,
    pub(crate) sid: String,
    pub(crate) email: String,
    pub(crate) api_key: String,
    pub(crate) paths: Paths,
    pub(crate) auth: CodexAuth,
    pub(crate) current_workspace: PathBuf,
    pub(crate) current_sessions_path: PathBuf,

    pub(crate) worker_tx: Sender<WorkerCmd>,
    pub(crate) worker_rx: Receiver<WorkerEvent>,
    pub(crate) provider_label: &'static str,
    pub(crate) frame_count: u64,
    pub(crate) tool_status: Option<String>,
    pub(crate) selecting_project: bool,
    pub(crate) selecting_session: bool,
    pub(crate) selecting_prompt: bool,
    pub(crate) selecting_settings: bool,
    pub(crate) editing_profile: bool,
    pub(crate) onboarding_active: bool,
    pub(crate) profile_field_index: usize,
    pub(crate) renaming_session: Option<usize>,
    pub(crate) renaming_project: Option<usize>,
    pub(crate) jobs_flash_until: u64,

    // Interaction state, refreshed each frame by render_*.
    pub(crate) menu_hits: Vec<(Rect, MenuAction)>,
    pub(crate) tab_hits: Vec<(Rect, usize)>,
    pub(crate) settings_hits: Vec<(Rect, usize)>,
    pub(crate) profile_hits: Vec<(Rect, usize)>,
    pub(crate) project_hits: Vec<(Rect, usize)>,
    pub(crate) session_hits: Vec<(Rect, usize)>,
    pub(crate) hovered_menu: Option<MenuAction>,
    pub(crate) hovered_settings: Option<usize>,
    pub(crate) hovered_profile: Option<usize>,
    pub(crate) hovered_project: Option<usize>,
    pub(crate) hovered_session: Option<usize>,
    pub(crate) pressed_menu: Option<MenuAction>,
}

impl App {
    pub const PROFILE_FIELDS: &'static [&'static str] = &[
        "First Name",
        "Last Name",
        "SID",
        "Email",
        "API Key",
    ];

    fn make_session(id: String, title: String) -> Session {
        Session {
            id,
            title,
            messages: Vec::new(),
            input: String::new(),
            pending: false,
            scroll: 0,
            pending_turn_id: None,
            pending_project_id: None,
            turn_steps: Vec::new(),
            jobs: Vec::new(),
        }
    }

    pub fn new(config: Config, paths: Paths, worker: WorkerHandles, auth: CodexAuth) -> Self {
        let mut app = Self {
            running: true,
            sessions: Vec::new(),
            projects: Vec::new(),
            active_project_id: None,
            active_session_id: None,
            active_tab: 0,
            user_name: config.user_name,
            assistant_name: config.assistant_name,
            default_model: config.default_model,
            global_prompt: config.global_prompt,
            show_jobs_pane: config.show_jobs_pane,
            onboarding_complete: config.onboarding_complete,
            first_name: config.first_name,
            last_name: config.last_name,
            sid: config.sid,
            email: config.email,
            api_key: config.api_key,
            current_workspace: paths.workspace.clone(),
            current_sessions_path: paths.sessions.clone(),
            paths,
            auth,
            worker_tx: worker.cmd_tx,
            worker_rx: worker.event_rx,
            provider_label: worker.provider_label,
            frame_count: 0,
            tool_status: None,
            selecting_project: false,
            selecting_session: false,
            selecting_prompt: false,
            selecting_settings: false,
            editing_profile: false,
            onboarding_active: false,
            profile_field_index: 0,
            renaming_session: None,
            renaming_project: None,
            jobs_flash_until: 0,
            menu_hits: Vec::new(),
            tab_hits: Vec::new(),
            settings_hits: Vec::new(),
            profile_hits: Vec::new(),
            project_hits: Vec::new(),
            session_hits: Vec::new(),
            hovered_menu: None,
            hovered_settings: None,
            hovered_profile: None,
            hovered_project: None,
            hovered_session: None,
            pressed_menu: None,
        };

        app.load_sessions().ok();
        app.load_projects().ok();

        if app.sessions.is_empty() {
            let id = Uuid::new_v4().to_string();
            app.sessions.push(Self::make_session(id.clone(), "chat 1".into()));
            app.active_session_id = Some(id);
        } else {
            app.active_session_id = app.sessions.first().map(|s| s.id.clone());
        }

        if !app.onboarding_complete {
            app.onboarding_active = true;
            app.editing_profile = true;
        }

        app
    }

    pub fn load_sessions(&mut self) -> Result<()> {
        let dir = &self.current_sessions_path;
        self.sessions.clear();
        if !dir.exists() { return Ok(()); }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("yaml") {
                let text = fs::read_to_string(&path)?;
                if let Ok(session) = serde_yaml::from_str::<Session>(&text) {
                    self.sessions.push(Self::compacted_session_snapshot(&session));
                }
            }
        }
        self.sessions.sort_by(|a, b| a.title.cmp(&b.title));
        
        // Sync active_tab with active_session_id if possible
        if let Some(id) = &self.active_session_id {
            if let Some(idx) = self.sessions.iter().position(|s| s.id == *id) {
                self.active_tab = idx;
            }
        }
        Ok(())
    }

    pub fn save_active_session(&self) -> Result<()> {
        if let Some(session) = self.sessions.get(self.active_tab) {
            fs::create_dir_all(&self.current_sessions_path)?;
            let path = self.current_sessions_path.join(format!("{}.yaml", session.id));
            let text = serde_yaml::to_string(&Self::compacted_session_snapshot(session))?;
            fs::write(path, text)?;
        }
        Ok(())
    }

    pub fn save_config(&self) -> Result<()> {
        let config = Config {
            user_name: self.user_name.clone(),
            assistant_name: self.assistant_name.clone(),
            default_model: self.default_model.clone(),
            global_prompt: self.global_prompt.clone(),
            show_jobs_pane: self.show_jobs_pane,
            onboarding_complete: self.onboarding_complete,
            first_name: self.first_name.clone(),
            last_name: self.last_name.clone(),
            sid: self.sid.clone(),
            email: self.email.clone(),
            api_key: self.api_key.clone(),
        };
        let text = serde_yaml::to_string(&config)?;
        fs::write(&self.paths.config_file, text)?;
        Ok(())
    }

    pub fn profile_field_value(&self, idx: usize) -> &str {
        match idx {
            0 => &self.first_name,
            1 => &self.last_name,
            2 => &self.sid,
            3 => &self.email,
            4 => &self.api_key,
            _ => "",
        }
    }

    pub fn profile_field_value_mut(&mut self, idx: usize) -> Option<&mut String> {
        match idx {
            0 => Some(&mut self.first_name),
            1 => Some(&mut self.last_name),
            2 => Some(&mut self.sid),
            3 => Some(&mut self.email),
            4 => Some(&mut self.api_key),
            _ => None,
        }
    }

    pub fn finish_profile_edit(&mut self) {
        self.onboarding_complete = true;
        self.onboarding_active = false;
        self.editing_profile = false;
        self.selecting_settings = false;
        self.profile_field_index = 0;
        self.save_config().ok();
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
                    if let Ok(project) = serde_yaml::from_str::<Project>(&text) {
                        self.projects.push(project);
                    }
                }
            }
        }
        self.projects.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(())
    }

    pub fn save_active_project(&mut self) -> Result<()> {
        if let Some(id) = &self.active_project_id {
            self.save_project_by_id(id)?;
            self.save_active_session().ok();
        }
        Ok(())
    }

    fn save_project_by_id(&self, project_id: &str) -> Result<()> {
        if let Some(project) = self.projects.iter().find(|p| p.id == project_id) {
            let dir = self.paths.projects.join(&project.id);
            fs::create_dir_all(dir.join("workspace"))?;
            fs::create_dir_all(dir.join("sessions"))?;
            let path = dir.join("project.yaml");
            let text = serde_yaml::to_string(project)?;
            fs::write(path, text)?;
        }
        Ok(())
    }

    pub fn switch_project(&mut self, id: Option<String>) -> Result<()> {
        if self.active_project_id.is_some() { self.save_active_project().ok(); }
        else { self.save_active_session().ok(); }

        self.active_project_id = id;
        self.sessions.clear();
        self.active_tab = 0;
        self.active_session_id = None;
        
        // Reset UI states
        self.selecting_project = false;
        self.selecting_session = false;
        self.selecting_prompt = false;
        self.selecting_settings = false;
        self.renaming_session = None;
        self.renaming_project = None;

        let (workspace, sessions_path) = if let Some(id) = &self.active_project_id {
            let project_dir = self.paths.projects.join(id);
            (project_dir.join("workspace"), project_dir.join("sessions"))
        } else {
            (self.paths.workspace.clone(), self.paths.sessions.clone())
        };

        self.current_workspace = workspace.clone();
        self.current_sessions_path = sessions_path;
        self.load_sessions().ok();

        if self.sessions.is_empty() {
            let session_id = Uuid::new_v4().to_string();
            self.sessions
                .push(Self::make_session(session_id.clone(), "chat 1".into()));
            self.active_session_id = Some(session_id);
        } else {
            self.active_session_id = self.sessions.first().map(|s| s.id.clone());
        }

        let canon_workspace = fs::canonicalize(&workspace).unwrap_or(workspace);
        let provider = Box::new(CodexProvider::new(self.auth.clone(), canon_workspace)?);
        let _ = self.worker_tx.send(WorkerCmd::UpdateProvider { provider });
        Ok(())
    }

    pub fn new_project(&mut self) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        let name = format!("proj-{}", &id[..4]);
        self.tool_status = Some(format!("Creating {}...", name));
        let project = Project {
            id: id.clone(),
            name: name.clone(),
            board: Board::default(),
            prompt: None,
        };
        let dir = self.paths.projects.join(&id);
        fs::create_dir_all(dir.join("workspace"))?;
        fs::create_dir_all(dir.join("sessions"))?;
        let path = dir.join("project.yaml");
        let text = serde_yaml::to_string(&project)?;
        fs::write(path, text)?;
        self.projects.push(project);
        self.switch_project(Some(id))?;
        self.tool_status = None;
        Ok(())
    }

    pub fn close_session(&mut self, idx: usize) {
        self.sessions.remove(idx);
        if self.sessions.is_empty() {
            self.sessions
                .push(Self::make_session(Uuid::new_v4().to_string(), "chat 1".into()));
        }
        if self.active_tab >= self.sessions.len() {
            self.active_tab = self.sessions.len().saturating_sub(1);
        }
        self.active_session_id = self.sessions.get(self.active_tab).map(|s| s.id.clone());
        if self.active_project_id.is_some() { self.save_active_project().ok(); }
    }

    pub fn delete_session(&mut self, idx: usize) {
        let session = self.sessions.remove(idx);
        let path = self.current_sessions_path.join(format!("{}.yaml", session.id));
        fs::remove_file(path).ok();
        
        if self.sessions.is_empty() {
            let session_id = Uuid::new_v4().to_string();
            self.sessions
                .push(Self::make_session(session_id.clone(), "chat 1".into()));
            self.active_session_id = Some(session_id);
        }
        
        if self.active_tab >= self.sessions.len() {
            self.active_tab = self.sessions.len().saturating_sub(1);
        }
        self.active_session_id = self.sessions.get(self.active_tab).map(|s| s.id.clone());
        
        if self.active_project_id.is_some() { self.save_active_project().ok(); }
        else { self.save_active_session().ok(); }
    }

    pub fn new_session(&mut self) {
        let mut n = self.sessions.len() + 1;
        let mut title = format!("chat {}", n);
        while self.sessions.iter().any(|s| s.title == title) {
            n += 1;
            title = format!("chat {}", n);
        }
        let id = Uuid::new_v4().to_string();
        self.sessions.push(Self::make_session(id.clone(), title.clone()));
        self.active_tab = self.sessions.len() - 1;
        self.active_session_id = Some(id);
        self.selecting_session = false;
        if self.active_project_id.is_some() { self.save_active_project().ok(); }
        else { self.save_active_session().ok(); }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press { return; }

        if self.editing_profile {
            match key.code {
                KeyCode::Esc if !self.onboarding_active => {
                    self.editing_profile = false;
                    self.selecting_settings = false;
                    self.profile_field_index = 0;
                    self.save_config().ok();
                }
                KeyCode::Up => {
                    self.profile_field_index = self.profile_field_index.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Tab => {
                    self.profile_field_index =
                        (self.profile_field_index + 1).min(Self::PROFILE_FIELDS.len() - 1);
                }
                KeyCode::BackTab => {
                    self.profile_field_index = self.profile_field_index.saturating_sub(1);
                }
                KeyCode::Enter => {
                    if self.profile_field_index + 1 >= Self::PROFILE_FIELDS.len() {
                        self.finish_profile_edit();
                    } else {
                        self.profile_field_index += 1;
                    }
                }
                KeyCode::Backspace => {
                    if let Some(value) = self.profile_field_value_mut(self.profile_field_index) {
                        value.pop();
                    }
                }
                KeyCode::Delete => {
                    if let Some(value) = self.profile_field_value_mut(self.profile_field_index) {
                        value.clear();
                    }
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    if let Some(value) = self.profile_field_value_mut(self.profile_field_index) {
                        value.push(c);
                    }
                }
                _ => {}
            }
            return;
        }
        
        // Global escape from renaming/overlays
        if key.code == KeyCode::Esc {
            self.selecting_project = false;
            self.selecting_session = false;
            self.selecting_prompt = false;
            self.selecting_settings = false;
            self.hovered_settings = None;
            self.renaming_session = None;
            self.renaming_project = None;
            return;
        }

        if let Some(idx) = self.renaming_session {
            match key.code {
                KeyCode::Enter | KeyCode::Esc => { self.renaming_session = None; if self.active_project_id.is_some() { self.save_active_project().ok(); } else { self.save_active_session().ok(); } }
                KeyCode::Backspace => { if let Some(s) = self.sessions.get_mut(idx) { s.title.pop(); } }
                KeyCode::Char(c) => { if let Some(s) = self.sessions.get_mut(idx) { s.title.push(c); } }
                _ => {}
            }
            return;
        }

        if let Some(idx) = self.renaming_project {
            match key.code {
                KeyCode::Enter | KeyCode::Esc => { self.renaming_project = None; self.save_active_project().ok(); }
                KeyCode::Backspace => { if let Some(p) = self.projects.get_mut(idx) { p.name.pop(); } }
                KeyCode::Char(c) => { if let Some(p) = self.projects.get_mut(idx) { p.name.push(c); } }
                _ => {}
            }
            return;
        }

        if self.selecting_prompt {
            match key.code {
                KeyCode::Esc => { self.selecting_prompt = false; }
                KeyCode::Enter => { 
                    if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) {
                        if let Some(id) = &self.active_project_id {
                            let p = self.projects.iter_mut().find(|p| p.id == *id).unwrap();
                            if p.prompt.is_none() { p.prompt = Some(self.global_prompt.clone()); }
                            p.prompt.as_mut().unwrap().push('\n');
                        } else {
                            self.global_prompt.push('\n');
                        }
                    } else {
                        self.selecting_prompt = false; 
                    }
                }
                KeyCode::Backspace => {
                    if let Some(id) = &self.active_project_id {
                        let p = self.projects.iter_mut().find(|p| p.id == *id).unwrap();
                        if p.prompt.is_none() { p.prompt = Some(self.global_prompt.clone()); }
                        p.prompt.as_mut().unwrap().pop();
                    } else {
                        self.global_prompt.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(id) = &self.active_project_id {
                        let p = self.projects.iter_mut().find(|p| p.id == *id).unwrap();
                        if p.prompt.is_none() { p.prompt = Some(self.global_prompt.clone()); }
                        p.prompt.as_mut().unwrap().push(c);
                    } else {
                        self.global_prompt.push(c);
                    }
                }
                _ => {}
            }
            return;
        }

        match (key.code, key.modifiers) {
            (KeyCode::Tab, _) => { self.next_tab(); return; }
            (KeyCode::BackTab, _) => { self.prev_tab(); return; }
            (KeyCode::Char('n'), m) if m.contains(KeyModifiers::CONTROL) => {
                self.new_session();
                return;
            }
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                    if self.selecting_session {
                        if let Some(idx) = self.hovered_session {
                            if idx < self.sessions.len() { self.renaming_session = Some(idx); }
                        }
                    } else if self.selecting_settings {
                        self.editing_profile = true;
                        self.profile_field_index = 0;
                    } else if self.selecting_project {
                        if let Some(idx) = self.hovered_project {
                            if idx > 0 && idx <= self.projects.len() { self.renaming_project = Some(idx - 1); }
                        }
                } else {
                    self.renaming_session = Some(self.active_tab);
                }
                return;
            }
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
        if text == "/new" {
            session.input.clear();
            self.new_session();
            return;
        }
        let turn_id = Uuid::new_v4().to_string();
        session.input.clear();
        session.messages.push(Message { role: Role::User, body: text, tool_calls: None });
        session.messages.push(Message { role: Role::Assistant, body: String::new(), tool_calls: None });
        session.pending = true;
        session.pending_turn_id = Some(turn_id.clone());
        session.pending_project_id = self.active_project_id.clone();
        session.turn_steps.clear();
        let session_id = session.id.clone();
        let mut messages: Vec<ProviderMessage> = session.messages.iter()
            .filter(|m| !(matches!(m.role, Role::Assistant) && m.body.is_empty() && m.tool_calls.is_none()))
            .map(|m| ProviderMessage { 
                role: m.role, 
                content: m.body.clone(), 
                tool_calls: m.tool_calls.clone() 
            }).collect();
        messages.retain(|m| !m.content.is_empty() || matches!(m.role, Role::Assistant) || m.tool_calls.is_some());
        
        let board = self.active_project_id.as_ref()
            .and_then(|id| self.projects.iter().find(|p| p.id == *id).map(|p| p.board.clone()));
        
        let custom_prompt = if let Some(id) = &self.active_project_id {
            self.projects.iter().find(|p| p.id == *id).and_then(|p| p.prompt.clone()).or(Some(self.global_prompt.clone()))
        } else {
            Some(self.global_prompt.clone())
        };

        let request = ProviderRequest {
            messages,
            model: self.default_model.clone(),
            project_id: self.active_project_id.clone(),
            board,
            custom_prompt,
            agent: AgentKind::Orchestrator,
        };
        let _ = self.worker_tx.send(WorkerCmd::Send { turn_id, session_id, request });
        if self.active_project_id.is_some() { self.save_active_project().ok(); }
        else { self.save_active_session().ok(); }
        self.scroll_to_bottom();
    }

    pub fn scroll_to_bottom(&mut self) {
        if let Some(session) = self.sessions.get_mut(self.active_tab) {
            // Very aggressive scroll to bottom. The UI will clamp it.
            session.scroll = 9999;
        }
    }

    fn session_for_turn_mut(&mut self, session_id: &str, turn_id: &str) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|session| {
            session.id == session_id && session.pending_turn_id.as_deref() == Some(turn_id)
        })
    }

    fn finalize_session_turn(session: &mut Session) {
        session.pending = false;
        session.pending_turn_id = None;
        session.pending_project_id = None;
        *session = Self::compacted_session_snapshot(session);
    }

    fn compacted_session_snapshot(session: &Session) -> Session {
        let mut snapshot = session.clone();
        snapshot.pending = false;
        snapshot.pending_turn_id = None;
        snapshot.pending_project_id = None;
        snapshot.turn_steps.clear();

        for message in &mut snapshot.messages {
            if matches!(message.role, Role::Assistant) {
                message.tool_calls = None;
            }
        }

        snapshot.messages.retain(|message| {
            !matches!(message.role, Role::ToolResult)
                && !(matches!(message.role, Role::Assistant) && message.body.trim().is_empty())
        });
        snapshot
    }

    fn save_after_event(&self, project_id: Option<&str>, session_id: &str) {
        if let Some(project_id) = project_id {
            self.save_project_by_id(project_id).ok();
        } else if self.active_project_id.is_none()
            && self
                .sessions
                .get(self.active_tab)
                .map(|session| session.id.as_str() == session_id)
                .unwrap_or(false)
        {
            self.save_active_session().ok();
        }
    }

    pub fn drain_worker_events(&mut self) {
        while let Ok(ev) = self.worker_rx.try_recv() {
            match ev {
                WorkerEvent::Delta { session_id, turn_id, delta } => {
                    if let Some(s) = self.session_for_turn_mut(&session_id, &turn_id) {
                        if let Some(last_assistant) = s.messages.iter_mut().rev().find(|m| matches!(m.role, Role::Assistant)) {
                            last_assistant.body.push_str(&delta);
                            self.scroll_to_bottom();
                        }
                    }
                }
                WorkerEvent::Done { session_id, turn_id } => {
                    let mut project_to_save = None;
                    if let Some(s) = self.session_for_turn_mut(&session_id, &turn_id) {
                        project_to_save = s.pending_project_id.clone();
                        Self::finalize_session_turn(s);
                        self.tool_status = None;
                        self.scroll_to_bottom();
                    }
                    self.save_after_event(project_to_save.as_deref(), &session_id);
                }
                WorkerEvent::SystemNote { session_id, turn_id, note } => {
                    if let Some(s) = self.session_for_turn_mut(&session_id, &turn_id) {
                        if s.pending { self.tool_status = Some(note); }
                    }
                }
                WorkerEvent::ToolStatus { session_id, turn_id, status } => {
                    if let Some(s) = self.session_for_turn_mut(&session_id, &turn_id) {
                        if s.pending { self.tool_status = Some(status); }
                    }
                }
                WorkerEvent::ToolCalls { session_id, turn_id, calls } => {
                    if let Some(s) = self.session_for_turn_mut(&session_id, &turn_id) {
                        if let Some(last_assistant) = s.messages.iter_mut().rev().find(|m| matches!(m.role, Role::Assistant)) {
                            let has_args = calls.as_array()
                                .map(|a| a.iter().any(|c| !c.pointer("/function/arguments").unwrap_or(&serde_json::Value::Null).is_null()))
                                .unwrap_or(false);

                            if last_assistant.tool_calls.is_none() || has_args {
                                last_assistant.tool_calls = Some(calls);
                            }
                        }
                        self.scroll_to_bottom();
                    }
                }
                WorkerEvent::PhaseChange { session_id, turn_id, phase } => {
                    if let Some(s) = self.session_for_turn_mut(&session_id, &turn_id) {
                        upsert_turn_step(
                            &mut s.turn_steps,
                            phase,
                            TurnStepStatus::Running,
                            None,
                        );
                        self.tool_status = Some(match phase {
                            AgentPhase::Plan => "planning...".into(),
                            AgentPhase::Explore => "exploring...".into(),
                            AgentPhase::Act => "executing...".into(),
                            AgentPhase::Verify => "verifying...".into(),
                            AgentPhase::Respond => "responding...".into(),
                        });
                    }
                }
                WorkerEvent::StepUpdate { session_id, turn_id, phase, status, summary } => {
                    if let Some(s) = self.session_for_turn_mut(&session_id, &turn_id) {
                        upsert_turn_step(&mut s.turn_steps, phase, status, summary);
                    }
                }
                WorkerEvent::StepArtifact { session_id, turn_id, phase, artifact } => {
                    if let Some(s) = self.session_for_turn_mut(&session_id, &turn_id) {
                        append_turn_artifact(&mut s.turn_steps, phase, artifact);
                    }
                }
                WorkerEvent::BoardUpdate { session_id, turn_id, project_id, operations } => {
                    if self.session_for_turn_mut(&session_id, &turn_id).is_none() {
                        continue;
                    }
                    if let Some(project_id) = project_id {
                        if let Some(project) = self.projects.iter_mut().find(|p| p.id == project_id) {
                            project.board.apply_operations(&operations);
                            self.save_project_by_id(&project_id).ok();
                        }
                    }
                }
                WorkerEvent::JobStarted { session_id, job } => {
                    let project_to_save = job.project_id.clone();
                    if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                        session.jobs.push(job);
                    }
                    self.save_after_event(project_to_save.as_deref(), &session_id);
                }
                WorkerEvent::JobUpdated { session_id, job_id, status, summary } => {
                    let mut project_to_save = None;
                    if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                        if let Some(job) = session.jobs.iter_mut().find(|job| job.id == job_id) {
                            job.status = status;
                            job.summary = summary;
                            project_to_save = job.project_id.clone();
                        }
                        if matches!(status, JobStatus::Running) {
                            self.tool_status = Some(format!("background job: {}", job_id.chars().take(8).collect::<String>()));
                        } else if matches!(status, JobStatus::Completed | JobStatus::Failed) {
                            self.jobs_flash_until = self.frame_count.saturating_add(36);
                        }
                    }
                    self.save_after_event(project_to_save.as_deref(), &session_id);
                }
                WorkerEvent::JobMessage { session_id, job_id, content } => {
                    let mut project_to_save = None;
                    if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                        let message = session
                            .jobs
                            .iter()
                            .find(|job| job.id == job_id)
                            .map(|job| {
                                project_to_save = job.project_id.clone();
                                let prefix = match job.status {
                                    JobStatus::Completed => format!("[{} completed]\n", job.title),
                                    JobStatus::Failed => format!("[{} failed]\n", job.title),
                                    _ => format!("[{}]\n", job.title),
                                };
                                format!("{}{}", prefix, content)
                            })
                            .unwrap_or(content);
                        if !self.show_jobs_pane {
                            session.messages.push(Message {
                                role: Role::Assistant,
                                body: message,
                                tool_calls: None,
                            });
                            self.scroll_to_bottom();
                        }
                    }
                    self.save_after_event(project_to_save.as_deref(), &session_id);
                }
                WorkerEvent::Error { session_id, turn_id, err } => {
                    let mut project_to_save = None;
                    if let Some(s) = self.session_for_turn_mut(&session_id, &turn_id) {
                        project_to_save = s.pending_project_id.clone();
                        if let Some(last) = s.messages.last_mut() {
                            if matches!(last.role, Role::Assistant) && last.body.is_empty() { last.body = format!("[error] {}", err); }
                            else { s.messages.push(Message { role: Role::Assistant, body: format!("[error] {}", err), tool_calls: None }); }
                        }
                        Self::finalize_session_turn(s);
                        self.tool_status = None;
                        self.scroll_to_bottom();
                    }
                    self.save_after_event(project_to_save.as_deref(), &session_id);
                }
            }
        }
    }

    pub fn switch_session(&mut self, idx: usize) { 
        self.active_tab = idx; 
        self.active_session_id = self.sessions.get(idx).map(|s| s.id.clone());
        self.selecting_session = false; 
        self.scroll_to_bottom(); 
    }

    pub fn handle_mouse(&mut self, me: MouseEvent) {
        let pos = Position::new(me.column, me.row);
        match me.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => { self.hovered_menu = self.menu_hit(pos); self.hovered_settings = self.settings_hit(pos); self.hovered_profile = self.profile_hit(pos); self.hovered_project = self.project_hit(pos); self.hovered_session = self.session_hit(pos); }
            MouseEventKind::Down(btn) => {
                if btn == MouseButton::Left {
                    // Overlays first
                    if self.editing_profile {
                        if let Some(idx) = self.profile_hit(pos) {
                            self.profile_field_index = idx.min(Self::PROFILE_FIELDS.len() - 1);
                            return;
                        }
                    }
                    if self.selecting_settings {
                        if let Some(idx) = self.settings_hit(pos) {
                            if idx == 0 {
                                self.show_jobs_pane = !self.show_jobs_pane;
                                self.save_config().ok();
                            } else if idx == 1 {
                                self.editing_profile = true;
                                self.profile_field_index = 0;
                            }
                            if !self.editing_profile {
                                self.selecting_settings = false;
                            }
                            return;
                        }
                    }
                    if self.selecting_project {
                        if let Some(idx) = self.project_hit(pos) {
                            if idx == 0 { let _ = self.switch_project(None); }
                            else if idx == self.projects.len() + 1 { let _ = self.new_project(); }
                            else { 
                                let project_id = self.projects[idx - 1].id.clone();
                                let _ = self.switch_project(Some(project_id)); 
                            }
                            self.selecting_project = false;
                            return;
                        }
                    }
                    if self.selecting_session {
                        if let Some(idx) = self.session_hit(pos) {
                            if idx == self.sessions.len() { self.new_session(); }
                            else { self.switch_session(idx); }
                            return;
                        }
                    }

                    if let Some(action) = self.menu_hit(pos) { self.pressed_menu = Some(action); return; }
                    if let Some(idx) = self.tab_hit(pos) { 
                        self.active_tab = idx; 
                        self.active_session_id = self.sessions.get(idx).map(|s| s.id.clone());
                        return;
                    }
                } else if btn == MouseButton::Right {
                    if self.selecting_session {
                        if let Some(idx) = self.session_hit(pos) {
                            if idx < self.sessions.len() { self.delete_session(idx); return; }
                        }
                    }
                    if let Some(idx) = self.tab_hit(pos) { self.close_session(idx); return; }
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
    pub fn settings_hit(&self, p: Position) -> Option<usize> { self.settings_hits.iter().find(|(r, _)| r.contains(p)).map(|(_, i)| *i) }
    pub fn profile_hit(&self, p: Position) -> Option<usize> { self.profile_hits.iter().find(|(r, _)| r.contains(p)).map(|(_, i)| *i) }
    pub fn project_hit(&self, p: Position) -> Option<usize> { self.project_hits.iter().find(|(r, _)| r.contains(p)).map(|(_, i)| *i) }
    pub fn session_hit(&self, p: Position) -> Option<usize> { self.session_hits.iter().find(|(r, _)| r.contains(p)).map(|(_, i)| *i) }

    pub fn fire_menu(&mut self, action: MenuAction) {
        match action {
            MenuAction::Quit => self.running = false,
            MenuAction::Settings => { self.selecting_settings = !self.selecting_settings; self.selecting_project = false; self.selecting_session = false; self.selecting_prompt = false; }
            MenuAction::Projects => { self.selecting_project = !self.selecting_project; if self.selecting_project { self.load_projects().ok(); } self.selecting_settings = false; self.selecting_session = false; self.selecting_prompt = false; }
            MenuAction::Prompt => { self.selecting_prompt = !self.selecting_prompt; self.selecting_settings = false; self.selecting_project = false; self.selecting_session = false; }
            _ => {}
        }
    }

    pub fn next_tab(&mut self) { 
        if !self.sessions.is_empty() { 
            self.active_tab = (self.active_tab + 1) % self.sessions.len(); 
            self.active_session_id = self.sessions.get(self.active_tab).map(|s| s.id.clone());
        } 
    }
    pub fn prev_tab(&mut self) { 
        if !self.sessions.is_empty() { 
            let n = self.sessions.len(); 
            self.active_tab = (self.active_tab + n - 1) % n; 
            self.active_session_id = self.sessions.get(self.active_tab).map(|s| s.id.clone());
        } 
    }

    pub fn is_running(&self) -> bool { self.running }
}

fn upsert_turn_step(
    steps: &mut Vec<TurnStep>,
    phase: AgentPhase,
    status: TurnStepStatus,
    summary: Option<String>,
) {
    if let Some(step) = steps.iter_mut().find(|step| step.phase == phase) {
        step.status = status;
        if let Some(summary) = summary {
            step.summary = summary;
        }
        return;
    }

    steps.push(TurnStep {
        phase,
        status,
        summary: summary.unwrap_or_default(),
        artifacts: Vec::new(),
    });
}

fn append_turn_artifact(
    steps: &mut Vec<TurnStep>,
    phase: AgentPhase,
    artifact: ExecutionArtifact,
) {
    if let Some(step) = steps.iter_mut().find(|step| step.phase == phase) {
        step.artifacts.push(artifact);
        return;
    }

    steps.push(TurnStep {
        phase,
        status: TurnStepStatus::Running,
        summary: String::new(),
        artifacts: vec![artifact],
    });
}
