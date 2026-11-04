//! The Esc menu and the Options screen.

use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::widgets::{dim, val};
use crate::paths::tilde;
use crate::theme;
use crate::ui::app::App;
use crate::ui::menu::{section, Menu, OptionsState, MAIN_ITEMS, SECTIONS};

pub(super) fn draw_menu(f: &mut Frame, area: Rect, app: &App) {
    match &app.menu {
        Some(Menu::Main { selected }) => draw_main(f, area, *selected),
        Some(Menu::Options(st)) => draw_options(f, area, app, st),
        None => {}
    }
}

fn popup(area: Rect, w: u16, h: u16) -> Rect {
    let (w, h) = (w.min(area.width.saturating_sub(2)), h.min(area.height.saturating_sub(2)));
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

fn frame(title: &str, hint: &str) -> Block<'static> {
    Block::bordered()
        .border_type(theme::border_type())
        .border_style(Style::new().fg(theme::dim()))
        .title(Line::from(Span::styled(format!(" {title} "), Style::new().fg(theme::bright()).bold())))
        .title(Line::from(dim(format!(" {hint} "))).right_aligned())
        .style(Style::new().bg(theme::background()))
}

fn draw_main(f: &mut Frame, area: Rect, selected: usize) {
    let r = popup(area, 34, 11);
    f.render_widget(Clear, r);
    let block = frame("aitop", "esc");
    let inner = block.inner(r);
    f.render_widget(block, r);
    let mut lines = vec![Line::from("")];
    for (i, item) in MAIN_ITEMS.iter().enumerate() {
        let line = if i == selected {
            Line::from(vec![
                Span::styled("▶ ", Style::new().fg(theme::accent(crate::event::Tool::Claude))),
                Span::styled(format!(" {item} "), Style::new().fg(theme::bright()).bg(theme::select_bg()).bold()),
                Span::styled(" ◀", Style::new().fg(theme::accent(crate::event::Tool::Claude))),
            ])
        } else {
            Line::from(dim(format!("   {item}   ")))
        };
        lines.extend([line, Line::from("")]);
    }
    lines.push(Line::from(dim("↑↓ select · enter open")));
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

fn draw_options(f: &mut Frame, area: Rect, app: &App, st: &OptionsState) {
    let r = popup(area, 84, 24);
    f.render_widget(Clear, r);
    let block = frame("Options", "esc back");
    let inner = block.inner(r);
    f.render_widget(block, r);
    let [tabs, _, list, help, hints] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(4),
        Constraint::Length(1),
    ])
    .areas(inner);

    // Section tabs, and where changes go.
    let accent = theme::accent(crate::event::Tool::Claude);
    let mut spans = vec![Span::raw(" ")];
    for (i, s) in SECTIONS.iter().enumerate() {
        let style = if i == st.section {
            Style::new().fg(accent).bold().add_modifier(Modifier::UNDERLINED)
        } else {
            theme::dim_style()
        };
        spans.extend([Span::styled(format!(" {s} "), style), Span::raw(" ")]);
    }
    let used: usize = spans.iter().map(|s| s.width()).sum();
    f.render_widget(Paragraph::new(Line::from(spans)), tabs);
    // Where changes go, shortened from the left to fit beside the tabs.
    let room = (tabs.width as usize).saturating_sub(used + 2);
    let path = tilde(app.config_path());
    let label = "saved to ";
    if room > label.len() + 8 {
        let max = room - label.len() - 1;
        let n = path.chars().count();
        let path = if n > max { format!("…{}", path.chars().skip(n + 1 - max).collect::<String>()) } else { path };
        f.render_widget(Paragraph::new(dim(format!("{label}{path} "))).alignment(Alignment::Right), tabs);
    }

    // Settings and their values.
    let items = section(st.section);
    let name_w = items.iter().map(|s| s.name.len()).max().unwrap_or(10) + 4;
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let selected = i == st.selected;
            let value = match (&st.editing, selected) {
                (Some(buf), true) => Span::styled(format!(" {buf}▏"), Style::new().fg(theme::bright()).bold()),
                _ if selected => Span::styled(format!("◀ {} ▶", (s.show)(app)), Style::new().fg(accent).bold()),
                _ => val(format!("  {}", (s.show)(app))),
            };
            let name = format!("  {:<name_w$}", s.name);
            let line = Line::from(vec![
                Span::styled(
                    name,
                    if selected { Style::new().fg(theme::bright()).bold() } else { theme::label_style() },
                ),
                value,
            ]);
            if selected {
                line.style(Style::new().bg(theme::select_bg()))
            } else {
                line
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines), list);

    // Help for the selected setting, or the last error.
    if let Some(s) = items.get(st.selected) {
        let mut text = vec![Line::from(dim(s.help))];
        if let Some(e) = &st.error {
            text.insert(0, Line::from(Span::styled(e.clone(), Style::new().fg(theme::bad()))));
        }
        let help = help.inner(Margin { horizontal: 1, vertical: 0 });
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), help);
    }
    let hint = if st.editing.is_some() {
        " type a value · enter to save · esc to cancel"
    } else if items.get(st.selected).is_some_and(|s| s.editable()) {
        " ↑↓ select · ←→ change · enter type a value · tab next section · esc back"
    } else {
        " ↑↓ select · ←→ or enter change · tab next section · esc back"
    };
    f.render_widget(Paragraph::new(dim(hint)), hints);
}
