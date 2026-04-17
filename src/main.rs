use std::io::{stdout, Stdout};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;

mod agent;
mod api;
mod app;
mod auth;
mod config;
mod models;
mod tools;
mod ui;

use agent::provider::{CodexProvider, EchoProvider, Provider};
use api::CodexClient;
use app::App;
use tools::Sandbox;

type Tui = Terminal<CrosstermBackend<Stdout>>;

const FRAME_BUDGET: Duration = Duration::from_micros(16_667);
const MAX_EVENTS_PER_FRAME: u32 = 64;

fn setup_terminal() -> Result<Tui> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(out))?)
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
    while app.is_running() {
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
                _ => {}
            }
            events_handled += 1;
            if !app.is_running() {
                break;
            }
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    install_panic_hook();
    let (cfg, paths) = config::bootstrap()?;
    // Legacy startup wiring still selects between Codex Responses transport and
    // the echo fallback directly. Step 13 will replace this with explicit,
    // configurable provider selection for migration providers.
    let (provider, auth): (Box<dyn Provider>, _) = match auth::CodexAuth::load_from_disk() {
        Ok(auth) => (
            Box::new(CodexProvider::new(auth.clone(), paths.workspace.clone())?),
            auth,
        ),
        Err(_) => (Box::new(EchoProvider), auth::CodexAuth::default()),
    };
    let worker = agent::worker::spawn_worker(Arc::from(provider));

    // Start background cron scheduler. This still uses the legacy Codex-backed
    // transport client, but now through the shared transport abstraction layer in
    // `src/api/` instead of provider-owned request plumbing.
    if let (Ok(client), Ok(sandbox)) = (CodexClient::new(), Sandbox::new(paths.workspace.clone())) {
        let _cron_handle = agent::cron_scheduler::start_cron_scheduler(
            auth.clone(),
            client,
            sandbox,
            cfg.default_model.clone(),
            worker.event_tx.clone(),
        );
    }

    let mut terminal = setup_terminal()?;
    let mut app = App::new(cfg, paths, worker, auth);
    let result = run(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    result
}
