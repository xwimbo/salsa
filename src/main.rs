use std::io::{stdout, Stdout};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use std::sync::mpsc::{Receiver, Sender};
use tui_tabs::TabNav;

mod auth;
mod config;
mod provider;

use config::{Config, Paths};
use provider::{
    CodexProvider, EchoProvider, Provider, ProviderMessage, ProviderRequest, WorkerCmd, WorkerEvent,
    WorkerHandles,
};

type Tui = Terminal<CrosstermBackend<Stdout>>;

const FRAME_BUDGET: Duration = Duration::from_micros(16_667);
const MAX_EVENTS_PER_FRAME: u32 = 64;

mod theme {
    use ratatui::style::Color;
    pub const BG: Color = Color::Rgb(0x0d, 0x11, 0x17);
    pub const BG_ALT: Color = Color::Rgb(0x16, 0x1b, 0x22);
    pub const FG: Color = Color::Rgb(0xc9, 0xd1, 0xd9);
    pub const MUTED: Color = Color::Rgb(0x8b, 0x94, 0x9e);
    pub const BORDER: Color = Color::Rgb(0x30, 0x36, 0x3d);
    pub const ORANGE: Color = Color::Rgb(0xe3, 0xb3, 0x41);
    pub const RED: Color = Color::Rgb(0xf8, 0x51, 0x49);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MenuAction {
    Settings,
    Sessions,
    Agent,
    Projects,
    Help,
    Quit,
}

impl MenuAction {
    const ALL: &'static [(&'static str, MenuAction)] = &[
        ("Settings", MenuAction::Settings),
        ("Sessions", MenuAction::Sessions),
        ("Agent", MenuAction::Agent),
        ("Projects", MenuAction::Projects),
        ("Help", MenuAction::Help),
        ("Quit", MenuAction::Quit),
    ];
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Role {
    User,
    Assistant,
}

#[derive(Clone)]
struct Message {
    role: Role,
    body: String,
}

struct Session {
    id: u64,
    title: String,
    messages: Vec<Message>,
    input: String,
    pending: bool,
}

struct App {
    running: bool,
    sessions: Vec<Session>,
    #[allow(dead_code)]
    next_session_id: u64,
    active_tab: usize,
    user_name: String,
    assistant_name: String,
    default_model: String,
    paths: Paths,

    worker_tx: Sender<WorkerCmd>,
    worker_rx: Receiver<WorkerEvent>,
    provider_label: &'static str,
    frame_count: u64,

    // Interaction state, refreshed each frame by render_*.
    menu_hits: Vec<(Rect, MenuAction)>,
    tab_hits: Vec<(Rect, usize)>,
    hovered_menu: Option<MenuAction>,
    pressed_menu: Option<MenuAction>,
}

impl App {
    fn new(config: Config, paths: Paths, worker: WorkerHandles) -> Self {
        let mut next_session_id = 1u64;
        let mut make_session = |title: &str| {
            let id = next_session_id;
            next_session_id += 1;
            Session {
                id,
                title: title.into(),
                messages: Vec::new(),
                input: String::new(),
                pending: false,
            }
        };
        let sessions = vec![make_session("chat 1"), make_session("chat 2")];
        Self {
            running: true,
            sessions,
            next_session_id,
            active_tab: 0,
            user_name: config.user_name,
            assistant_name: config.assistant_name,
            default_model: config.default_model,
            paths,
            worker_tx: worker.cmd_tx,
            worker_rx: worker.event_rx,
            provider_label: worker.provider_label,
            frame_count: 0,
            menu_hits: Vec::new(),
            tab_hits: Vec::new(),
            hovered_menu: None,
            pressed_menu: None,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        // Quit + tab cycling always work, even while the assistant is thinking.
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.running = false;
                return;
            }
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.running = false;
                return;
            }
            (KeyCode::Tab, _) => {
                self.next_tab();
                return;
            }
            (KeyCode::BackTab, _) => {
                self.prev_tab();
                return;
            }
            _ => {}
        }

        if self.active_session_pending() {
            return;
        }

        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => self.submit(),
            (KeyCode::Backspace, _) => {
                if let Some(s) = self.sessions.get_mut(self.active_tab) {
                    s.input.pop();
                }
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) => {
                if let Some(s) = self.sessions.get_mut(self.active_tab) {
                    s.input.push(c);
                }
            }
            _ => {}
        }
    }

    fn active_session_pending(&self) -> bool {
        self.sessions
            .get(self.active_tab)
            .map(|s| s.pending)
            .unwrap_or(false)
    }

    fn submit(&mut self) {
        let Some(session) = self.sessions.get_mut(self.active_tab) else {
            return;
        };
        let text = session.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        session.input.clear();
        session.messages.push(Message {
            role: Role::User,
            body: text,
        });
        session.messages.push(Message {
            role: Role::Assistant,
            body: String::new(),
        });
        session.pending = true;
        let session_id = session.id;

        // Snapshot the conversation for the worker, excluding the empty
        // assistant placeholder we just pushed.
        let mut messages: Vec<ProviderMessage> = session
            .messages
            .iter()
            .filter(|m| !(matches!(m.role, Role::Assistant) && m.body.is_empty()))
            .map(|m| ProviderMessage {
                role: m.role,
                content: m.body.clone(),
            })
            .collect();
        // Guard against any other accidental empties.
        messages.retain(|m| !m.content.is_empty() || matches!(m.role, Role::Assistant));

        let request = ProviderRequest {
            messages,
            model: self.default_model.clone(),
        };
        let _ = self.worker_tx.send(WorkerCmd::Send {
            session_id,
            request,
        });
    }

    fn drain_worker_events(&mut self) {
        while let Ok(ev) = self.worker_rx.try_recv() {
            match ev {
                WorkerEvent::Delta { session_id, delta } => {
                    if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                        if let Some(last) = s.messages.last_mut() {
                            if matches!(last.role, Role::Assistant) {
                                last.body.push_str(&delta);
                            }
                        }
                    }
                }
                WorkerEvent::Done { session_id } => {
                    if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                        s.pending = false;
                    }
                }
                WorkerEvent::Error { session_id, err } => {
                    if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                        if let Some(last) = s.messages.last_mut() {
                            if matches!(last.role, Role::Assistant) && last.body.is_empty() {
                                last.body = format!("[error] {}", err);
                            } else {
                                s.messages.push(Message {
                                    role: Role::Assistant,
                                    body: format!("[error] {}", err),
                                });
                            }
                        }
                        s.pending = false;
                    }
                }
            }
        }
    }

    fn handle_mouse(&mut self, me: MouseEvent) {
        let pos = Position::new(me.column, me.row);
        match me.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                self.hovered_menu = self.menu_hit(pos);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(action) = self.menu_hit(pos) {
                    self.pressed_menu = Some(action);
                }
                if let Some(idx) = self.tab_hit(pos) {
                    self.active_tab = idx;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(pressed) = self.pressed_menu.take() {
                    if self.menu_hit(pos) == Some(pressed) {
                        self.fire_menu(pressed);
                    }
                }
            }
            MouseEventKind::ScrollUp => self.prev_tab(),
            MouseEventKind::ScrollDown => self.next_tab(),
            _ => {}
        }
    }

    fn menu_hit(&self, p: Position) -> Option<MenuAction> {
        self.menu_hits
            .iter()
            .find(|(r, _)| r.contains(p))
            .map(|(_, a)| *a)
    }

    fn tab_hit(&self, p: Position) -> Option<usize> {
        self.tab_hits
            .iter()
            .find(|(r, _)| r.contains(p))
            .map(|(_, i)| *i)
    }

    fn fire_menu(&mut self, action: MenuAction) {
        match action {
            MenuAction::Quit => self.running = false,
            // Other menus are placeholders until their respective steps land.
            _ => {}
        }
    }

    fn next_tab(&mut self) {
        if !self.sessions.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.sessions.len();
        }
    }

    fn prev_tab(&mut self) {
        if !self.sessions.is_empty() {
            let n = self.sessions.len();
            self.active_tab = (self.active_tab + n - 1) % n;
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>) {
        self.frame_count = self.frame_count.wrapping_add(1);
        let area = frame.area();

        frame.render_widget(
            Block::default().style(Style::default().bg(theme::BG)),
            area,
        );

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // menu bar
                Constraint::Length(3), // tab bar (TabNav requires 3 rows)
                Constraint::Min(1),    // chat
                Constraint::Length(5), // input
            ])
            .split(area);

        self.render_menu(frame, chunks[0]);
        self.render_tabs(frame, chunks[1]);
        self.render_chat(frame, chunks[2]);
        self.render_input(frame, chunks[3]);
    }

    fn render_menu(&mut self, frame: &mut Frame<'_>, area: Rect) {
        self.menu_hits.clear();

        frame.render_widget(
            Block::default().style(Style::new().bg(theme::BG_ALT)),
            area,
        );

        let mut x = area.x + 1;
        for (label, action) in MenuAction::ALL {
            let text = format!(" {} ", label);
            let width = text.chars().count() as u16;
            if x + width > area.x + area.width {
                break;
            }
            let rect = Rect {
                x,
                y: area.y,
                width,
                height: 1,
            };

            let hovered = self.hovered_menu == Some(*action);
            let pressed = self.pressed_menu == Some(*action);
            let style = if pressed {
                Style::new()
                    .fg(theme::BG)
                    .bg(theme::ORANGE)
                    .add_modifier(Modifier::BOLD)
            } else if hovered {
                Style::new()
                    .fg(theme::ORANGE)
                    .bg(theme::BG_ALT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(theme::FG).bg(theme::BG_ALT)
            };

            let para = Paragraph::new(Line::from(Span::styled(text, style)));
            frame.render_widget(para, rect);

            self.menu_hits.push((rect, *action));
            x += width + 1;
        }

        // Right-aligned status: which provider is live + workspace location.
        let status = format!(
            "mode: {}  •  workspace: {}",
            self.provider_label,
            config::tilde_path(&self.paths.workspace)
        );
        let status_width = status.chars().count() as u16;
        if status_width + 1 < area.width && x + status_width + 1 < area.x + area.width {
            let status_rect = Rect {
                x: area.x + area.width - status_width - 1,
                y: area.y,
                width: status_width,
                height: 1,
            };
            let para = Paragraph::new(Line::from(Span::styled(
                status,
                Style::new().fg(theme::MUTED).bg(theme::BG_ALT),
            )));
            frame.render_widget(para, status_rect);
        }
    }

    fn render_tabs(&mut self, frame: &mut Frame<'_>, area: Rect) {
        self.tab_hits.clear();

        // Only render as many tabs as fit while leaving at least 2 cols of
        // buffer before the right edge, so the chat container's top-right
        // rounded corner has a clean place to sit.
        let mut offset = 0u16;
        let mut fit_count = 0usize;
        for s in self.sessions.iter() {
            // tui-tabs: each tab is adjacent, width = label.len() + 8
            let w = s.title.chars().count() as u16 + 8;
            if offset + w + 2 > area.width {
                break;
            }
            self.tab_hits.push((
                Rect {
                    x: area.x + offset,
                    y: area.y,
                    width: w,
                    height: area.height,
                },
                fit_count,
            ));
            offset += w;
            fit_count += 1;
        }

        if fit_count > 0 {
            let titles: Vec<&str> = self
                .sessions
                .iter()
                .take(fit_count)
                .map(|s| s.title.as_str())
                .collect();
            let selected = self.active_tab.min(fit_count - 1);
            let tabs = TabNav::new(&titles, selected)
                .style(Style::new().fg(theme::MUTED))
                .highlight_style(Style::new().fg(theme::ORANGE).add_modifier(Modifier::BOLD))
                .border_style(Style::new().fg(theme::BORDER));

            let tabs_area = Rect {
                x: area.x,
                y: area.y,
                width: offset,
                height: area.height,
            };
            frame.render_widget(tabs, tabs_area);
        }

        // Patch the baseline row so it becomes a proper top edge for the
        // chat container below:
        //   - left cell: `├` if there are tabs (junction), else plain `╭`
        //   - middle cells between last tab and the right edge: `─` extension
        //   - rightmost cell: `╮` (top-right rounded corner)
        if area.width >= 2 && area.height >= 3 {
            let baseline_y = area.y + area.height - 1;
            let right = area.x + area.width - 1;
            let tab_end = area.x + offset;

            let buf = frame.buffer_mut();
            let border_style = Style::new().fg(theme::BORDER).bg(theme::BG);

            // When tab 0 is active, tui-tabs "opens" its baseline under that
            // tab, so a junction `├` would imply a horizontal line going right
            // that isn't there. A plain `│` reads as the active tab's left
            // edge flowing down into the chat container's left border.
            let left_symbol = if fit_count == 0 {
                "╭"
            } else if self.active_tab == 0 {
                "│"
            } else {
                "├"
            };
            if let Some(cell) = buf.cell_mut(Position::new(area.x, baseline_y)) {
                cell.set_symbol(left_symbol);
                cell.set_style(border_style);
            }
            for cx in tab_end..right {
                if let Some(cell) = buf.cell_mut(Position::new(cx, baseline_y)) {
                    cell.set_symbol("─");
                    cell.set_style(border_style);
                }
            }
            if let Some(cell) = buf.cell_mut(Position::new(right, baseline_y)) {
                cell.set_symbol("╮");
                cell.set_style(border_style);
            }
        }
    }

    fn render_chat(&mut self, frame: &mut Frame<'_>, area: Rect) {
        // Top border comes from TabNav's baseline above, so we only draw
        // left/right/bottom borders here.
        let block = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::BORDER));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(session) = self.sessions.get(self.active_tab) else {
            return;
        };

        let mut lines: Vec<Line> = Vec::new();
        for (i, msg) in session.messages.iter().enumerate() {
            if i > 0 {
                lines.push(Line::from(""));
            }
            let (label, color) = match msg.role {
                Role::User => (format!("{{ {} }}", self.user_name), theme::RED),
                Role::Assistant => (format!("{{ {} }}", self.assistant_name), theme::ORANGE),
            };
            lines.push(Line::from(Span::styled(
                label,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )));
            for body_line in msg.body.lines() {
                lines.push(Line::from(Span::styled(
                    body_line.to_string(),
                    Style::default().fg(theme::FG),
                )));
            }
        }

        let padded = Rect {
            x: inner.x + 1,
            y: inner.y,
            width: inner.width.saturating_sub(2),
            height: inner.height,
        };

        let para = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(para, padded);
    }

    fn render_input(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let pending = self.active_session_pending();
        let border_color = if pending {
            pending_border_color(self.frame_count)
        } else {
            theme::BORDER
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let padded = Rect {
            x: inner.x + 1,
            y: inner.y,
            width: inner.width.saturating_sub(2),
            height: inner.height,
        };

        if pending {
            let span = Span::styled(
                "thinking…",
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::ITALIC),
            );
            frame.render_widget(Paragraph::new(Line::from(span)), padded);
            return;
        }

        let content = self
            .sessions
            .get(self.active_tab)
            .map(|s| s.input.as_str())
            .unwrap_or("");

        let line = if content.is_empty() {
            Line::from(Span::styled(
                "type a message…",
                Style::default().fg(theme::MUTED),
            ))
        } else {
            Line::from(vec![
                Span::styled(content.to_string(), Style::default().fg(theme::FG)),
                Span::styled("▏", Style::default().fg(theme::ORANGE)),
            ])
        };

        frame.render_widget(Paragraph::new(line), padded);
    }
}

fn pending_border_color(frame_count: u64) -> Color {
    // Rotate through a small palette every ~6 frames (~100ms at 60fps).
    const COLORS: [Color; 5] = [
        theme::ORANGE,
        theme::RED,
        Color::Rgb(0x58, 0xa6, 0xff), // GH dark blue
        Color::Rgb(0x3f, 0xb9, 0x50), // GH dark green
        Color::Rgb(0xbc, 0x8c, 0xff), // GH dark purple
    ];
    COLORS[((frame_count / 6) as usize) % COLORS.len()]
}

fn setup_terminal() -> Result<Tui> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(out);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture);
        default(info);
    }));
}

fn run(terminal: &mut Tui, app: &mut App) -> Result<()> {
    let mut next_frame = Instant::now() + FRAME_BUDGET;
    while app.running {
        app.drain_worker_events();
        terminal.draw(|frame| app.render(frame))?;

        let mut events_handled = 0u32;
        loop {
            let now = Instant::now();
            if now >= next_frame {
                if now.saturating_duration_since(next_frame) > FRAME_BUDGET * 4 {
                    next_frame = now;
                }
                next_frame += FRAME_BUDGET;
                break;
            }
            if events_handled >= MAX_EVENTS_PER_FRAME {
                break;
            }
            let timeout = next_frame - now;
            if !event::poll(timeout)? {
                continue;
            }
            match event::read()? {
                Event::Key(key) => app.handle_key(key),
                Event::Mouse(me) => app.handle_mouse(me),
                Event::Resize(_, _) => {}
                _ => {}
            }
            events_handled += 1;
            if !app.running {
                break;
            }
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    install_panic_hook();
    let (cfg, paths) = config::bootstrap()?;

    let provider: Box<dyn Provider> = match auth::CodexAuth::load_from_disk()
        .and_then(|auth| CodexProvider::new(auth))
    {
        Ok(p) => Box::new(p),
        Err(_) => Box::new(EchoProvider),
    };
    let worker = provider::spawn_worker(provider);

    let mut terminal = setup_terminal()?;
    let mut app = App::new(cfg, paths, worker);
    let result = run(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    result
}
