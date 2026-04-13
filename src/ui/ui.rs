use ratatui::{layout::*, style::*, text::*, widgets::*, Frame};
use crate::{app::*, config, models::*};

impl App {
    pub fn render(&mut self, f: &mut Frame<'_>) {
        self.frame_count = self.frame_count.wrapping_add(1);
        let area = f.area();
        f.render_widget(Block::new().bg(theme::BG), area);
        
        let c = Layout::vertical([Constraint::Length(1), Constraint::Length(3), Constraint::Min(1), Constraint::Length(5)]).split(area);
        self.render_menu(f, c[0]);
        self.render_tabs(f, c[1]);
        self.render_chat(f, c[2]);
        self.render_input(f, c[3]);
        self.render_overlays(f, area);
    }

    fn render_overlays(&mut self, f: &mut Frame<'_>, area: Rect) {
        let mut draw_pop = |r: Rect, title: &str, items: Vec<(String, usize, bool)>, hits: &mut Vec<(Rect, usize)>| {
            f.render_widget(Clear, r);
            f.render_widget(Block::bordered().title(title).border_type(BorderType::Rounded).fg(theme::ORANGE), r);
            let inner = r.inner(Margin { horizontal: 1, vertical: 1 });
            hits.clear();
            for (i, (label, id, hovered)) in items.into_iter().enumerate() {
                let rect = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
                if rect.y >= inner.bottom() { break; }
                let s = if hovered { Style::new().bg(theme::BG_ALT).fg(theme::ORANGE) } else { Style::new().fg(theme::FG) };
                f.render_widget(Paragraph::new(label).style(s), rect);
                hits.push((rect, id));
            }
        };

        if self.selecting_project {
            let r = Rect { x: area.x + 5, y: area.y + 1, width: 30, height: (self.projects.len() as u16 + 4).min(area.height - 2) };
            let mut items = vec![(" [Global Sessions]".to_owned(), 0, self.hovered_project == Some(0))];
            for (i, p) in self.projects.iter().enumerate() {
                let name = if p.name.len() > 15 { format!("{}…", &p.name[..14]) } else { p.name.clone() };
                items.push((format!(" {}", if self.renaming_project == Some(i) { format!("*{}", name) } else { name }), i + 1, self.hovered_project == Some(i + 1)));
            }
            items.push((" + New Project...".to_owned(), self.projects.len() + 1, self.hovered_project == Some(self.projects.len() + 1)));
            draw_pop(r, "", items, &mut self.project_hits);
        }

        if self.selecting_session {
            let r = Rect { x: area.x + 15, y: area.y + 1, width: 30, height: (self.sessions.len() as u16 + 3).min(area.height - 2) };
            let mut items: Vec<_> = self.sessions.iter().enumerate().map(|(i, s)| {
                let t = s.title.chars().take(6).collect::<String>();
                (format!(" {}", if self.renaming_session == Some(i) { format!("*{}", t) } else { t }), i, self.hovered_session == Some(i))
            }).collect();
            items.push((" + New Session...".to_owned(), self.sessions.len(), self.hovered_session == Some(self.sessions.len())));
            draw_pop(r, "", items, &mut self.session_hits);
        }

        if self.selecting_prompt {
            let r = Rect { x: area.x + 10, y: area.y + 5, width: area.width.saturating_sub(20), height: area.height.saturating_sub(10) };
            f.render_widget(Clear, r);
            let title = self.active_project_id.and_then(|id| self.projects.iter().find(|p| p.id == id)).map(|p| p.name.as_str()).unwrap_or("Global");
            let block = Block::bordered().title(format!(" {} Prompt ", title)).border_type(BorderType::Rounded).fg(theme::ORANGE);
            let text = self.active_project_id.and_then(|id| self.projects.iter().find(|p| p.id == id)).and_then(|p| p.prompt.as_deref()).unwrap_or(&self.global_prompt);
            f.render_widget(Paragraph::new(text).block(block).wrap(Wrap { trim: false }), r);
        }
    }

    fn render_menu(&mut self, f: &mut Frame<'_>, area: Rect) {
        self.menu_hits.clear();
        f.render_widget(Block::new().bg(theme::BG_ALT), area);
        let mut x = area.x + 1;
        for (label, action) in MenuAction::ALL {
            let text = format!(" {} ", label);
            let w = text.len() as u16;
            let style = match (self.hovered_menu, self.pressed_menu) {
                (Some(a), _) if a == *action => Style::new().fg(theme::ORANGE).bg(theme::BG_ALT).bold(),
                (_, Some(a)) if a == *action => Style::new().fg(theme::BG).bg(theme::ORANGE).bold(),
                _ => Style::new().fg(theme::FG).bg(theme::BG_ALT),
            };
            let rect = Rect { x, y: area.y, width: w, height: 1 };
            f.render_widget(Paragraph::new(text).style(style), rect);
            self.menu_hits.push((rect, *action));
            x += w + 1;
        }
        let ws = config::tilde_path(&self.current_workspace);
        let status = format!("mode: {} • ws: {}", self.provider_label, if ws.len() > 10 { format!("…{}", &ws[ws.len()-8..]) } else { ws });
        let s_rect = Rect { x: area.right().saturating_sub(status.len() as u16 + 1), y: area.y, width: status.len() as u16, height: 1 };
        f.render_widget(Paragraph::new(status).fg(theme::MUTED), s_rect);
    }

    fn render_tabs(&mut self, f: &mut Frame<'_>, area: Rect) {
        self.tab_hits.clear();
        if self.sessions.is_empty() { return; }
        let mut x = 0;
        for (i, s) in self.sessions.iter().enumerate() {
            let t = if self.renaming_session == Some(i) { format!("*{}", s.title) } else { s.title.clone() };
            let label = format!(" {} ", t.chars().take(6).collect::<String>());
            let w = label.len() as u16 + 2;
            if x + w > area.width { break; }
            let rect = Rect { x: area.x + x, y: area.y, width: w, height: 3 };
            let (fg, bfg) = if i == self.active_tab { (theme::ORANGE, theme::ORANGE) } else { (theme::MUTED, theme::BORDER) };
            f.render_widget(Paragraph::new(label).block(Block::bordered().border_type(BorderType::Rounded).fg(bfg)).style(Style::new().fg(fg)), rect);
            self.tab_hits.push((rect, i));
            x += w;
        }
    }

    fn render_chat(&mut self, f: &mut Frame<'_>, area: Rect) {
        let block = Block::bordered().border_type(BorderType::Rounded).fg(theme::BORDER);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let Some(s) = self.sessions.get_mut(self.active_tab) else { return };
        let mut lines = Vec::new();
        for msg in &s.messages {
            let (n, c) = if msg.role == Role::User { (&self.user_name, theme::RED) } else { (&self.assistant_name, theme::ORANGE) };
            lines.push(Line::from(Span::styled(format!("{{ {} }}", n), Style::new().fg(c).bold())));
            msg.body.lines().for_each(|l| lines.push(Line::from(l)));
        }
        let p = Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((s.scroll, 0));
        f.render_widget(p, inner.inner(Margin { horizontal: 1, vertical: 0 }));
    }

    fn render_input(&mut self, f: &mut Frame<'_>, area: Rect) {
        let pending = self.active_session_pending();
        let border = if pending { theme::pending_border_color(self.frame_count) } else { theme::BORDER };
        let block = Block::bordered().border_type(BorderType::Rounded).fg(border);
        let inner = block.inner(area).inner(Margin { horizontal: 1, vertical: 0 });
        f.render_widget(block, area);
        if pending {
            let status = self.tool_status.as_deref().unwrap_or("thinking…");
            f.render_widget(Paragraph::new(status).italic().fg(theme::MUTED), inner);
        } else if let Some(s) = self.sessions.get(self.active_tab) {
            let l = if s.input.is_empty() { Line::from(Span::styled("type...", Style::new().fg(theme::MUTED))) }
                    else { Line::from(vec![Span::raw(&s.input), Span::styled("▏", Style::new().fg(theme::ORANGE))]) };
            f.render_widget(Paragraph::new(l), inner);
        }
    }
}