pub mod theme;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};
use ratatui::Frame;
// use tui_tabs::TabNav;

use crate::app::{App, MenuAction};
use crate::config;
use crate::models::Role;

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
        self.render_chat(frame, chunks[2]);
        self.render_input(frame, chunks[3]);
        self.render_overlays(frame, area);
    }

    fn render_overlays(&mut self, frame: &mut Frame<'_>, area: Rect) {
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
            frame.render_widget(Paragraph::new(" [Global Sessions]").style(gs), gr);
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
                
                let mut display_name = if project.name.len() > 15 { format!("{}…", &project.name[..14]) } else { project.name.clone() };
                if self.renaming_project == Some(idx) {
                    display_name = format!("*{}", display_name);
                }
                frame.render_widget(Paragraph::new(format!(" {}", display_name)).style(style), rect);
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
                frame.render_widget(Paragraph::new(format!(" {}", display_title)).style(style), rect);
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
                let name = self.projects.iter().find(|p| p.id == *id).map(|p| p.name.as_str()).unwrap_or("?");
                format!("Project Prompt: {}", name) 
            } else { "Global Prompt".to_string() };
            let block = Block::default()
                .title(format!(" {} ", title))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme::ORANGE));
            let inner = block.inner(overlay);
            frame.render_widget(block, overlay);
            
            let prompt_text = if let Some(id) = &self.active_project_id {
                self.projects.iter().find(|p| p.id == *id).and_then(|p| p.prompt.as_deref()).unwrap_or(&self.global_prompt)
            } else {
                &self.global_prompt
            };
            
            frame.render_widget(Paragraph::new(prompt_text).wrap(Wrap { trim: false }), inner);
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
            let name = self.projects.iter().find(|p| p.id == *id).map(|p| p.name.as_str()).unwrap_or("?");
            format!("project: {}  •  ", name)
        } else {
            "global  •  ".to_string()
        };
        let workspace_path = config::tilde_path(&self.current_workspace);
        let status = format!(
            "{}mode: {}  •  workspace: {}",
            project_prefix,
            self.provider_label,
            workspace_path
        );
        let mut status_width = status.chars().count() as u16;
        let available_status_width = area.width.saturating_sub(x - area.x + 2);
        
        if status_width > available_status_width && available_status_width > 10 {
            // Truncate workspace path more aggressively
            let truncated_workspace = if workspace_path.len() > 10 {
                format!("…{}", &workspace_path[workspace_path.len().saturating_sub(8)..])
            } else {
                workspace_path.clone()
            };
            let mut new_status = format!(
                "{}mode: {}  •  ws: {}",
                project_prefix,
                self.provider_label,
                truncated_workspace
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
            if i > 0 { w_total += 3; } // for «
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
            let rect = Rect { x: area.x + x_offset, y: area.y, width: 3, height: 3 };
            frame.render_widget(Paragraph::new("«").block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(theme::BORDER))), rect);
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
                Style::default().fg(theme::ORANGE).add_modifier(Modifier::BOLD)
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
            
            frame.render_widget(Paragraph::new(format!(" {} ", display_title)).block(block).style(style), rect);
            
            x_offset += w;
            last_rendered_idx = i;
        }

        if last_rendered_idx < self.sessions.len() - 1 {
            let rect = Rect { x: area.x + x_offset, y: area.y, width: 3, height: 3 };
            frame.render_widget(Paragraph::new("»").block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(theme::BORDER))), rect);
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
                        lines.push(Line::from(Span::styled("(empty message)", Style::default().fg(theme::MUTED).add_modifier(Modifier::ITALIC))));
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
                                    let name = call.pointer("/function/name").and_then(|v| v.as_str()).unwrap_or("?");
                                    lines.push(Line::from(Span::styled(
                                        format!("  ▸ tool: {}", name),
                                        Style::default().fg(theme::MUTED).add_modifier(Modifier::ITALIC),
                                    )));
                                }
                            }
                        } else if !has_text {
                            lines.push(Line::from(Span::styled("  (thinking...)", Style::default().fg(theme::MUTED).add_modifier(Modifier::ITALIC))));
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
            let status = self.tool_status.as_deref().unwrap_or("thinking…");
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    status,
                    Style::default().fg(theme::MUTED).add_modifier(Modifier::ITALIC),
                ))),
                padded,
            );
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
