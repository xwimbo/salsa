pub mod theme;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    Wrap,
};
use ratatui::Frame;
// use tui_tabs::TabNav;

use crate::app::{App, FileBrowserButton, MenuAction};
use crate::config;
use crate::models::{AgentKind, ExecutionArtifact, JobStatus, Role, TurnStepStatus};

impl App {
    pub fn render(&mut self, frame: &mut Frame<'_>) {
        self.frame_count = self.frame_count.wrapping_add(1);
        let area = frame.area();
        frame.render_widget(Block::default().style(Style::default().bg(theme::BG)), area);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(5),
            ])
            .split(area);
        self.render_menu(frame, chunks[0]);
        self.render_tabs(frame, chunks[1]);
        let content_chunks = if self.should_show_jobs_pane(chunks[2]) {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(1), Constraint::Length(32)])
                .split(chunks[2])
        } else {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(1), Constraint::Length(0)])
                .split(chunks[2])
        };
        self.render_chat(frame, content_chunks[0]);
        if content_chunks[1].width > 0 {
            self.render_jobs(frame, content_chunks[1]);
        }
        self.render_input(frame, chunks[3]);
        self.render_overlays(frame, area);
    }

    fn should_show_jobs_pane(&self, area: Rect) -> bool {
        self.show_jobs_pane
            && area.width >= 80
            && self
                .sessions
                .get(self.active_tab)
                .map(|session| !session.jobs.is_empty())
                .unwrap_or(false)
    }

    fn render_overlays(&mut self, frame: &mut Frame<'_>, area: Rect) {
        if self.selecting_settings {
            let overlay = Rect {
                x: area.x + 1,
                y: area.y + 1,
                width: 34,
                height: 5,
            };
            frame.render_widget(Clear, overlay);
            let block = Block::default()
                .title(" Settings ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme::ORANGE));
            let inner = block.inner(overlay);
            frame.render_widget(block, overlay);
            self.settings_hits.clear();
            let rect0 = Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 1,
            };
            let style = if self.hovered_settings == Some(0) {
                Style::default().bg(theme::BG_ALT).fg(theme::ORANGE)
            } else {
                Style::default().fg(theme::FG)
            };
            let label = if self.show_jobs_pane {
                "[x] Show Jobs Queue"
            } else {
                "[ ] Show Jobs Queue"
            };
            frame.render_widget(Paragraph::new(label).style(style), rect0);
            self.settings_hits.push((rect0, 0));
            let rect1 = Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: 1,
            };
            let style1 = if self.hovered_settings == Some(1) {
                Style::default().bg(theme::BG_ALT).fg(theme::ORANGE)
            } else {
                Style::default().fg(theme::FG)
            };
            frame.render_widget(Paragraph::new("Edit Profile").style(style1), rect1);
            self.settings_hits.push((rect1, 1));
        }

        if self.editing_profile {
            let overlay = Rect {
                x: area.x + 6,
                y: area.y + 3,
                width: area.width.saturating_sub(12).max(48),
                height: 13,
            };
            frame.render_widget(Clear, overlay);
            let title = if self.onboarding_active {
                " Welcome Setup "
            } else {
                " Profile Settings "
            };
            let block = Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme::ORANGE));
            let inner = block.inner(overlay);
            frame.render_widget(block, overlay);
            self.profile_hits.clear();

            let intro = if self.onboarding_active {
                "First run setup. Add your profile details. Enter saves on the last field."
            } else {
                "Edit stored profile fields. `Delete` clears a field. `Esc` closes."
            };
            frame.render_widget(
                Paragraph::new(intro).wrap(Wrap { trim: false }),
                Rect {
                    x: inner.x,
                    y: inner.y,
                    width: inner.width,
                    height: 2,
                },
            );

            for (idx, label) in App::PROFILE_FIELDS.iter().enumerate() {
                let row = Rect {
                    x: inner.x,
                    y: inner.y + 3 + idx as u16,
                    width: inner.width,
                    height: 1,
                };
                let active = self.profile_field_index == idx;
                let style = if active {
                    Style::default().bg(theme::BG_ALT).fg(theme::ORANGE)
                } else if self.hovered_profile == Some(idx) {
                    Style::default().bg(theme::BG_ALT).fg(theme::FG)
                } else {
                    Style::default().fg(theme::FG)
                };
                let mut value = self.profile_field_value(idx).to_string();
                if idx == 4 && !value.is_empty() {
                    value = "*".repeat(value.chars().count().min(24));
                }
                if active {
                    value.push('▏');
                }
                let field = format!("{label:>11}: {value}");
                frame.render_widget(Paragraph::new(field).style(style), row);
                self.profile_hits.push((row, idx));
            }

            let footer = if self.onboarding_active {
                "Up/Down to move. Enter advances. Finish on API Key to start."
            } else {
                "Up/Down to move. Enter advances. Finish on API Key to save."
            };
            frame.render_widget(
                Paragraph::new(footer).style(Style::default().fg(theme::MUTED)),
                Rect {
                    x: inner.x,
                    y: inner.bottom().saturating_sub(1),
                    width: inner.width,
                    height: 1,
                },
            );
        }
        if self.selecting_project {
            let overlay = Rect {
                x: area.x + 5,
                y: area.y + 1,
                width: 30,
                height: (self.projects.len() as u16 + 4).min(area.height - 2),
            };
            frame.render_widget(Clear, overlay);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme::ORANGE));
            let inner = block.inner(overlay);
            frame.render_widget(block, overlay);
            self.project_hits.clear();
            let mut y = inner.y;
            let gr = Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            };
            let gs = if self.hovered_project == Some(0) {
                Style::default().bg(theme::BG_ALT).fg(theme::ORANGE)
            } else {
                Style::default().fg(theme::FG)
            };
            frame.render_widget(Paragraph::new(" [Global Project]").style(gs), gr);
            self.project_hits.push((gr, 0));
            y += 1;
            for (idx, project) in self.projects.iter().enumerate() {
                if y >= inner.y + inner.height - 1 {
                    break;
                }
                let rect = Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: 1,
                };
                let style = if self.hovered_project == Some(idx + 1) {
                    Style::default().bg(theme::BG_ALT).fg(theme::ORANGE)
                } else {
                    Style::default().fg(theme::FG)
                };

                let mut display_name = if project.name.len() > 15 {
                    format!("{}…", &project.name[..14])
                } else {
                    project.name.clone()
                };
                if self.renaming_project == Some(idx) {
                    display_name = format!("*{}", display_name);
                }
                frame.render_widget(
                    Paragraph::new(format!(" {}", display_name)).style(style),
                    rect,
                );
                self.project_hits.push((rect, idx + 1));
                y += 1;
            }
            let nr = Rect {
                x: inner.x,
                y: inner.y + inner.height - 1,
                width: inner.width,
                height: 1,
            };
            let ns = if self.hovered_project == Some(self.projects.len() + 1) {
                Style::default().bg(theme::BG_ALT).fg(theme::ORANGE)
            } else {
                Style::default().fg(theme::MUTED)
            };
            frame.render_widget(Paragraph::new(" + New Project...").style(ns), nr);
            self.project_hits.push((nr, self.projects.len() + 1));
        }
        if self.selecting_session {
            let overlay = Rect {
                x: area.x + 15,
                y: area.y + 1,
                width: 30,
                height: (self.sessions.len() as u16 + 3).min(area.height - 2),
            };
            frame.render_widget(Clear, overlay);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme::ORANGE));
            let inner = block.inner(overlay);
            frame.render_widget(block, overlay);
            self.session_hits.clear();
            let mut y = inner.y;
            for (idx, session) in self.sessions.iter().enumerate() {
                if y >= inner.y + inner.height - 1 {
                    break;
                }
                let rect = Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: 1,
                };
                let style = if self.hovered_session == Some(idx) {
                    Style::default().bg(theme::BG_ALT).fg(theme::ORANGE)
                } else {
                    Style::default().fg(theme::FG)
                };
                let mut display_title = session.title.chars().take(6).collect::<String>();
                if self.renaming_session == Some(idx) {
                    display_title = format!("*{}", display_title);
                }
                frame.render_widget(
                    Paragraph::new(format!(" {}", display_title)).style(style),
                    rect,
                );
                self.session_hits.push((rect, idx));
                y += 1;
            }
            let nr = Rect {
                x: inner.x,
                y: inner.y + inner.height - 1,
                width: inner.width,
                height: 1,
            };
            let ns = if self.hovered_session == Some(self.sessions.len()) {
                Style::default().bg(theme::BG_ALT).fg(theme::ORANGE)
            } else {
                Style::default().fg(theme::MUTED)
            };
            frame.render_widget(Paragraph::new(" + New Session...").style(ns), nr);
            self.session_hits.push((nr, self.sessions.len()));
        }
        if self.selecting_prompt {
            let overlay = Rect {
                x: area.x + 10,
                y: area.y + 5,
                width: area.width.saturating_sub(20),
                height: area.height.saturating_sub(10),
            };
            frame.render_widget(Clear, overlay);
            let title = if let Some(id) = &self.active_project_id {
                let name = self
                    .projects
                    .iter()
                    .find(|p| p.id == *id)
                    .map(|p| p.name.as_str())
                    .unwrap_or("?");
                format!("Project Prompt: {}", name)
            } else {
                "Global Prompt".to_string()
            };
            let block = Block::default()
                .title(format!(" {} ", title))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme::ORANGE));
            let inner = block.inner(overlay);
            frame.render_widget(block, overlay);

            let prompt_text = if let Some(id) = &self.active_project_id {
                self.projects
                    .iter()
                    .find(|p| p.id == *id)
                    .and_then(|p| p.prompt.as_deref())
                    .unwrap_or(&self.global_prompt)
            } else {
                &self.global_prompt
            };

            frame.render_widget(
                Paragraph::new(prompt_text).wrap(Wrap { trim: false }),
                inner,
            );
        }

        if self.showing_help {
            let w = 58.min(area.width.saturating_sub(4));
            let h = 30.min(area.height.saturating_sub(4));
            let overlay = Rect {
                x: area.x + (area.width.saturating_sub(w)) / 2,
                y: area.y + (area.height.saturating_sub(h)) / 2,
                width: w,
                height: h,
            };
            frame.render_widget(Clear, overlay);
            let block = Block::default()
                .title(" Help ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme::ORANGE));
            let inner = block.inner(overlay);
            frame.render_widget(block, overlay);

            let help_lines = vec![
                Line::from(Span::styled(
                    "Keyboard Shortcuts",
                    Style::default().fg(theme::ORANGE).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                help_line("Ctrl+N", "New session/conversation"),
                help_line("Tab", "Next tab"),
                help_line("Shift+Tab", "Previous tab"),
                help_line("Ctrl+R", "Rename current session"),
                help_line("Enter", "Send message"),
                help_line("PageUp/Down", "Scroll chat history"),
                help_line("Esc", "Close overlay / cancel"),
                Line::from(""),
                Line::from(Span::styled(
                    "Slash Commands",
                    Style::default().fg(theme::ORANGE).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                help_line("/new session", "Create a new session"),
                help_line("/new project", "Create a new project"),
                help_line("/del session", "Delete current session"),
                help_line("/del project", "Delete current project"),
                help_line("/add", "Add files to workspace"),
                help_line("/help", "Show this help screen"),
                Line::from(""),
                Line::from(Span::styled(
                    "Menu Bar",
                    Style::default().fg(theme::ORANGE).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                help_line("Settings", "Toggle jobs pane, edit profile"),
                help_line("Prompt", "View/edit system prompt"),
                help_line("Projects", "Switch projects or create new"),
                help_line("Help", "This screen"),
                help_line("Quit", "Exit salsa"),
                Line::from(""),
                Line::from(Span::styled(
                    "Agent Tools",
                    Style::default().fg(theme::ORANGE).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                help_line("Filesystem", "Read, write, edit, delete files"),
                help_line("Shell", "Run commands in workspace"),
                help_line("Web Fetch", "Fetch and cache web pages"),
                help_line("Sub-agents", "Spawn background workers"),
                help_line("Teams", "Coordinate multiple agents"),
                help_line("Skills", "Load custom .md commands"),
                help_line("Cron", "Schedule recurring tasks"),
                help_line("Tasks/Todos", "Track work progress"),
                Line::from(""),
                Line::from(Span::styled(
                    "Config: ~/.salsa/config.yaml",
                    Style::default().fg(theme::MUTED),
                )),
                Line::from(Span::styled(
                    "Skills: ~/.salsa/commands/*.md",
                    Style::default().fg(theme::MUTED),
                )),
                Line::from(Span::styled(
                    "Press Esc to close",
                    Style::default().fg(theme::MUTED),
                )),
            ];

            frame.render_widget(
                Paragraph::new(help_lines).wrap(Wrap { trim: false }),
                Rect {
                    x: inner.x + 1,
                    y: inner.y,
                    width: inner.width.saturating_sub(2),
                    height: inner.height,
                },
            );
        }

        if let Some(fb) = &mut self.file_browser {
            let w = area.width.saturating_sub(8).min(80).max(48);
            let h = area.height.saturating_sub(6).min(24).max(12);
            let overlay = Rect {
                x: area.x + (area.width.saturating_sub(w)) / 2,
                y: area.y + (area.height.saturating_sub(h)) / 2,
                width: w,
                height: h,
            };
            frame.render_widget(Clear, overlay);
            let block = Block::default()
                .title(" Add Files ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme::ORANGE));
            let inner = block.inner(overlay);
            frame.render_widget(block, overlay);

            self.file_browser_hits.clear();
            self.file_browser_button_hits.clear();

            let path_line = Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 1,
            };
            frame.render_widget(
                Paragraph::new(format!("Directory: {}", config::tilde_path(&fb.current_dir)))
                    .style(Style::default().fg(theme::MUTED)),
                path_line,
            );

            let list_height = inner.height.saturating_sub(4);
            let list_area = Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: list_height,
            };

            if fb.cursor < fb.scroll_offset {
                fb.scroll_offset = fb.cursor;
            }
            let visible_rows = list_area.height as usize;
            if visible_rows > 0 && fb.cursor >= fb.scroll_offset + visible_rows {
                fb.scroll_offset = fb.cursor + 1 - visible_rows;
            }

            if fb.entries.is_empty() {
                frame.render_widget(
                    Paragraph::new("This folder is empty.")
                        .style(Style::default().fg(theme::MUTED)),
                    list_area,
                );
            } else {
                for row in 0..list_area.height {
                    let idx = fb.scroll_offset + row as usize;
                    if idx >= fb.entries.len() {
                        break;
                    }
                    let entry = &fb.entries[idx];
                    let rect = Rect {
                        x: list_area.x,
                        y: list_area.y + row,
                        width: list_area.width,
                        height: 1,
                    };
                    let selected = fb.is_selected(idx);
                    let cursor = idx == fb.cursor;
                    let hovered = self.hovered_file_browser == Some(idx);
                    let style = if cursor {
                        Style::default().bg(theme::BG_ALT).fg(theme::ORANGE)
                    } else if hovered {
                        Style::default().bg(theme::BG_ALT).fg(theme::FG)
                    } else {
                        Style::default().fg(theme::FG)
                    };
                    let marker = if entry.is_dir {
                        "›"
                    } else if selected {
                        "[x]"
                    } else {
                        "[ ]"
                    };
                    let label = if entry.is_dir {
                        format!("{marker} {}/", entry.name)
                    } else {
                        format!("{marker} {}", entry.name)
                    };
                    frame.render_widget(Paragraph::new(label).style(style), rect);
                    self.file_browser_hits.push((rect, idx));
                }
            }

            let footer_y = inner.y + inner.height.saturating_sub(2);
            let hint_rect = Rect {
                x: inner.x,
                y: footer_y,
                width: inner.width.saturating_sub(24),
                height: 1,
            };
            frame.render_widget(
                Paragraph::new("Enter=open/copy  Space=select  Backspace=up  Esc=cancel")
                    .style(Style::default().fg(theme::MUTED)),
                hint_rect,
            );

            let buttons = [
                (" Back ", FileBrowserButton::Back),
                (" Add Selected ", FileBrowserButton::Confirm),
                (" Cancel ", FileBrowserButton::Cancel),
            ];
            let total_button_width: u16 =
                buttons.iter().map(|(label, _)| label.chars().count() as u16).sum::<u16>() + 2;
            let mut x = inner.x + inner.width.saturating_sub(total_button_width);
            for (label, action) in buttons {
                let width = label.chars().count() as u16;
                let rect = Rect {
                    x,
                    y: footer_y,
                    width,
                    height: 1,
                };
                frame.render_widget(
                    Paragraph::new(label).style(Style::default().fg(theme::ORANGE)),
                    rect,
                );
                self.file_browser_button_hits.push((rect, action));
                x += width + 1;
            }
        } else {
            self.file_browser_hits.clear();
            self.file_browser_button_hits.clear();
        }
    }

    fn render_menu(&mut self, frame: &mut Frame<'_>, area: Rect) {
        self.menu_hits.clear();
        frame.render_widget(Block::default().style(Style::new().bg(theme::BG_ALT)), area);
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
            let style = if self.hovered_menu == Some(*action) {
                Style::new()
                    .fg(theme::ORANGE)
                    .bg(theme::BG_ALT)
                    .add_modifier(Modifier::BOLD)
            } else if self.pressed_menu == Some(*action) {
                Style::new()
                    .fg(theme::BG)
                    .bg(theme::ORANGE)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(theme::FG).bg(theme::BG_ALT)
            };
            frame.render_widget(Paragraph::new(Line::from(Span::styled(text, style))), rect);
            self.menu_hits.push((rect, *action));
            x += width + 1;
        }
        let project_prefix = if let Some(id) = &self.active_project_id {
            let name = self
                .projects
                .iter()
                .find(|p| p.id == *id)
                .map(|p| p.name.as_str())
                .unwrap_or("?");
            format!("project: {}  •  ", name)
        } else {
            "global  •  ".to_string()
        };
        let workspace_path = config::tilde_path(&self.current_workspace);
        let status = format!(
            "{}mode: {}  •  workspace: {}",
            project_prefix, self.provider_label, workspace_path
        );
        let mut status_width = status.chars().count() as u16;
        let available_status_width = area.width.saturating_sub(x - area.x + 2);

        if status_width > available_status_width && available_status_width > 10 {
            // Truncate workspace path more aggressively
            let truncated_workspace = if workspace_path.len() > 10 {
                format!(
                    "…{}",
                    &workspace_path[workspace_path.len().saturating_sub(8)..]
                )
            } else {
                workspace_path.clone()
            };
            let mut new_status = format!(
                "{}mode: {}  •  ws: {}",
                project_prefix, self.provider_label, truncated_workspace
            );
            status_width = new_status.chars().count() as u16;

            // If still too long, drop the prefix if we must, but try to keep it
            if status_width > available_status_width {
                new_status = format!("{} • ws: {}", self.provider_label, truncated_workspace);
                status_width = new_status.chars().count() as u16;
            }

            if status_width <= available_status_width {
                let status_rect = Rect {
                    x: area.x + area.width - status_width - 1,
                    y: area.y,
                    width: status_width,
                    height: 1,
                };
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        new_status,
                        Style::new().fg(theme::MUTED).bg(theme::BG_ALT),
                    ))),
                    status_rect,
                );
            }
        } else if status_width <= available_status_width {
            let status_rect = Rect {
                x: area.x + area.width - status_width - 1,
                y: area.y,
                width: status_width,
                height: 1,
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    status,
                    Style::new().fg(theme::MUTED).bg(theme::BG_ALT),
                ))),
                status_rect,
            );
        }
    }

    fn render_tabs(&mut self, frame: &mut Frame<'_>, area: Rect) {
        self.tab_hits.clear();
        if self.sessions.is_empty() {
            return;
        }
        let max_width = area.width;
        let mut start_idx = 0;

        // Find start_idx such that active_tab is visible and we fit as many as possible
        for i in 0..=self.active_tab {
            let mut w_total = 0;
            let mut active_fits = false;
            if i > 0 {
                w_total += 3;
            } // for «
            for j in i..self.sessions.len() {
                let mut display_title = self.sessions[j].title.chars().take(6).collect::<String>();
                if self.renaming_session == Some(j) {
                    display_title = format!("*{}", display_title);
                }
                let w = display_title.chars().count() as u16 + 4;
                let next_indicator_w = if j < self.sessions.len() - 1 { 3 } else { 0 };
                if w_total + w + next_indicator_w > max_width {
                    break;
                }
                w_total += w;
                if j == self.active_tab {
                    active_fits = true;
                }
            }
            if active_fits {
                start_idx = i;
                break;
            }
        }

        let mut x_offset = 0;
        if start_idx > 0 {
            let rect = Rect {
                x: area.x + x_offset,
                y: area.y,
                width: 3,
                height: 3,
            };
            frame.render_widget(
                Paragraph::new("«").block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(theme::BORDER)),
                ),
                rect,
            );
            x_offset += 3;
        }

        let mut last_rendered_idx = start_idx;
        for i in start_idx..self.sessions.len() {
            let mut display_title = self.sessions[i].title.chars().take(6).collect::<String>();
            if self.renaming_session == Some(i) {
                display_title = format!("*{}", display_title);
            }
            let w = display_title.chars().count() as u16 + 4;
            let next_indicator_w = if i < self.sessions.len() - 1 { 3 } else { 0 };
            if x_offset + w + next_indicator_w > max_width {
                break;
            }

            let is_active = i == self.active_tab;
            let style = if is_active {
                Style::default()
                    .fg(theme::ORANGE)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::MUTED)
            };
            let border_style = if is_active {
                Style::default().fg(theme::ORANGE)
            } else {
                Style::default().fg(theme::BORDER)
            };

            let rect = Rect {
                x: area.x + x_offset,
                y: area.y,
                width: w,
                height: 3,
            };
            self.tab_hits.push((rect, i));

            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border_style);

            frame.render_widget(
                Paragraph::new(format!(" {} ", display_title))
                    .block(block)
                    .style(style),
                rect,
            );

            x_offset += w;
            last_rendered_idx = i;
        }

        if last_rendered_idx < self.sessions.len() - 1 {
            let rect = Rect {
                x: area.x + x_offset,
                y: area.y,
                width: 3,
                height: 3,
            };
            frame.render_widget(
                Paragraph::new("»").block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(theme::BORDER)),
                ),
                rect,
            );
        }
    }

    fn render_chat(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::BORDER));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Use a scope to get a copy of the scroll value to avoid borrowing issues later
        let (current_session_scroll, active_tab) = {
            let Some(session) = self.sessions.get(self.active_tab) else {
                return;
            };
            (session.scroll, self.active_tab)
        };

        let mut lines: Vec<Line> = Vec::new();
        let session = &self.sessions[active_tab];
        for (i, msg) in session.messages.iter().enumerate() {
            if i > 0 {
                lines.push(Line::from(""));
            }
            match msg.role {
                Role::User => {
                    lines.push(Line::from(Span::styled(
                        format!("{{ {} }}", self.user_name),
                        Style::default().fg(theme::RED).add_modifier(Modifier::BOLD),
                    )));
                    if msg.body.is_empty() {
                        lines.push(Line::from(Span::styled(
                            "(empty message)",
                            Style::default()
                                .fg(theme::MUTED)
                                .add_modifier(Modifier::ITALIC),
                        )));
                    } else {
                        for bl in msg.body.lines() {
                            lines.push(Line::from(Span::styled(
                                bl.to_string(),
                                Style::default().fg(theme::FG),
                            )));
                        }
                    }
                }
                Role::Assistant => {
                    lines.push(Line::from(Span::styled(
                        format!("{{ {} }}", self.assistant_name),
                        Style::default()
                            .fg(theme::ORANGE)
                            .add_modifier(Modifier::BOLD),
                    )));
                    let mut has_text = false;
                    for bl in msg.body.lines() {
                        lines.push(Line::from(Span::styled(
                            bl.to_string(),
                            Style::default().fg(theme::FG),
                        )));
                        has_text = true;
                    }

                    // Show tool indicators only if the session is still pending AND this is the last message
                    let is_last_msg = i == session.messages.len() - 1;
                    if session.pending && is_last_msg {
                        if let Some(ref calls) = msg.tool_calls {
                            if let Some(calls_arr) = calls.as_array() {
                                for call in calls_arr {
                                    let name = call
                                        .pointer("/function/name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("?");
                                    lines.push(Line::from(Span::styled(
                                        format!("  ▸ tool: {}", name),
                                        Style::default()
                                            .fg(theme::MUTED)
                                            .add_modifier(Modifier::ITALIC),
                                    )));
                                }
                            }
                        } else if !has_text {
                            lines.push(Line::from(Span::styled(
                                "  (thinking...)",
                                Style::default()
                                    .fg(theme::MUTED)
                                    .add_modifier(Modifier::ITALIC),
                            )));
                        }
                    }
                }
                Role::System | Role::ToolResult => {
                    // Hidden in UI, these are internal turn state.
                }
            }
        }
        let padded = Rect {
            x: inner.x + 1,
            y: inner.y,
            width: inner.width.saturating_sub(2),
            height: inner.height,
        };
        let mut total_wrapped_height = 0;
        let available_width = padded.width as usize;
        if available_width > 0 {
            for line in &lines {
                let line_width = line.width();
                if line_width == 0 {
                    total_wrapped_height += 1;
                } else {
                    total_wrapped_height += (line_width + available_width - 1) / available_width;
                }
            }
        } else {
            total_wrapped_height = lines.len();
        }

        let max_scroll = total_wrapped_height.saturating_sub(padded.height as usize) as u16;
        let current_scroll = current_session_scroll.min(max_scroll);

        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((current_scroll, 0)),
            padded,
        );
        let mut scroll_state = ScrollbarState::default()
            .content_length(total_wrapped_height)
            .position(current_scroll as usize);
        frame.render_stateful_widget(
            Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓")),
            inner,
            &mut scroll_state,
        );

        // ONLY update the session scroll if it was clamped, otherwise let handle_mouse/key own it
        if let Some(s) = self.sessions.get_mut(active_tab) {
            if s.scroll > max_scroll {
                s.scroll = max_scroll;
            }
        }
    }

    fn render_jobs(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let border_style = if self.frame_count <= self.jobs_flash_until {
            Style::default().fg(theme::pending_border_color(self.frame_count))
        } else {
            Style::default().fg(theme::BORDER)
        };
        let block = Block::default()
            .title(" Jobs ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(session) = self.sessions.get(self.active_tab) else {
            return;
        };

        let mut lines: Vec<Line> = Vec::new();
        for job in session.jobs.iter().rev().take(8) {
            let status_marker = match job.status {
                JobStatus::Queued => "○",
                JobStatus::Running => "◌",
                JobStatus::Completed => "•",
                JobStatus::Failed => "!",
            };
            let status_style = match job.status {
                JobStatus::Queued => Style::default().fg(theme::MUTED),
                JobStatus::Running => Style::default()
                    .fg(theme::ORANGE)
                    .add_modifier(Modifier::BOLD),
                JobStatus::Completed => Style::default().fg(theme::FG),
                JobStatus::Failed => Style::default().fg(theme::RED).add_modifier(Modifier::BOLD),
            };
            let agent_label = match job.agent {
                AgentKind::Orchestrator => "orch",
                AgentKind::Planner => "plan",
                AgentKind::Coder => "code",
                AgentKind::Analyst => "data",
            };
            let mut title = job.title.clone();
            let max_title = inner.width.saturating_sub(8) as usize;
            if max_title > 0 && title.chars().count() > max_title {
                title = title
                    .chars()
                    .take(max_title.saturating_sub(1))
                    .collect::<String>();
                title.push('…');
            }
            lines.push(Line::from(vec![
                Span::styled(format!("{status_marker} "), status_style),
                Span::styled(
                    format!("{agent_label} "),
                    Style::default()
                        .fg(theme::ORANGE)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(title, Style::default().fg(theme::FG)),
            ]));
            if !job.summary.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  {}", job.summary),
                    Style::default().fg(theme::MUTED),
                )));
            }
            lines.push(Line::from(""));
        }

        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                "No jobs yet.",
                Style::default().fg(theme::MUTED),
            )));
        }

        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }),
            Rect {
                x: inner.x + 1,
                y: inner.y,
                width: inner.width.saturating_sub(2),
                height: inner.height,
            },
        );
    }

    fn render_input(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let pending = self.active_session_pending();
        let border_color = if pending {
            theme::pending_border_color(self.frame_count)
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
            let session = self.sessions.get(self.active_tab);
            let mut lines = Vec::new();
            let status = self.tool_status.as_deref().unwrap_or("thinking…");
            lines.push(Line::from(Span::styled(
                status,
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::ITALIC),
            )));
            if let Some(session) = session {
                for step in session.turn_steps.iter().rev().take(3).rev() {
                    let marker = match step.status {
                        TurnStepStatus::Running => "›",
                        TurnStepStatus::Completed => "•",
                        TurnStepStatus::Failed => "!",
                    };
                    let phase = match step.phase {
                        crate::models::AgentPhase::Plan => "plan",
                        crate::models::AgentPhase::Explore => "explore",
                        crate::models::AgentPhase::Act => "act",
                        crate::models::AgentPhase::Verify => "verify",
                        crate::models::AgentPhase::Respond => "respond",
                    };
                    let detail = if !step.summary.is_empty() {
                        step.summary.as_str()
                    } else {
                        latest_artifact_label(step.artifacts.last())
                    };
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{marker} {phase}: "),
                            Style::default()
                                .fg(theme::ORANGE)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(detail.to_string(), Style::default().fg(theme::FG)),
                    ]));
                }
            }
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), padded);
            return;
        }
        if let Some(ref confirm) = self.pending_confirm {
            let prompt = match confirm {
                crate::app::ConfirmAction::DeleteSession { title, .. } => {
                    format!("Delete session \"{}\"? (y/n)", title)
                }
                crate::app::ConfirmAction::DeleteProject { name } => {
                    format!("Delete project \"{}\"? (y/n)", name)
                }
            };
            let line = Line::from(vec![
                Span::styled(prompt, Style::default().fg(theme::RED).add_modifier(Modifier::BOLD)),
            ]);
            frame.render_widget(
                Paragraph::new(line).wrap(Wrap { trim: false }),
                padded,
            );
            return;
        }
        let content = self
            .sessions
            .get(self.active_tab)
            .map(|s| s.input.as_str())
            .unwrap_or("");
        let input_line = if content.is_empty() {
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
        frame.render_widget(
            Paragraph::new(input_line).wrap(Wrap { trim: false }),
            padded,
        );
    }
}

fn help_line<'a>(key: &'a str, desc: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("  {:14}", key),
            Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
        ),
        Span::styled(desc.to_string(), Style::default().fg(theme::MUTED)),
    ])
}

fn latest_artifact_label(artifact: Option<&ExecutionArtifact>) -> &'static str {
    match artifact {
        Some(ExecutionArtifact::AssistantNote { .. }) => "thinking",
        Some(ExecutionArtifact::ToolCall { .. }) => "calling tool",
        Some(ExecutionArtifact::ToolResult { .. }) => "tool returned",
        Some(ExecutionArtifact::BoardOps { .. }) => "board updated",
        None => "in progress",
    }
}
