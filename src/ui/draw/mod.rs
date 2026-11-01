//! Rendering. One module per screen region.

mod chart;
mod footer;
mod header;
mod help;
mod panel;
mod stats;
mod table;
mod widgets;

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Paragraph};
use ratatui::Frame;

use footer::draw_footer;
use header::draw_header;
use help::draw_help;
use panel::draw_panel;
use widgets::dim;

use super::app::App;
use crate::paths::tilde;
use crate::theme;

pub fn draw(f: &mut Frame, app: &mut App) {
    app.hits.clear();
    let area = f.area();
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    draw_header(f, header, app);
    draw_body(f, body, app);
    draw_footer(f, footer, app);
    if app.help {
        draw_help(f, area, app);
    }
}

fn draw_body(f: &mut Frame, area: Rect, app: &mut App) {
    if app.summary.is_none() {
        return;
    }
    let tools = if app.zoom { vec![app.focus] } else { app.visible() };
    let nothing_anywhere = !tools.iter().any(|&t| app.detected(t) || app.has_data(t));
    if nothing_anywhere && app.snap.loaded {
        draw_welcome(f, area, app);
        return;
    }
    let constraints: Vec<Constraint> = tools
        .iter()
        .map(|&t| if app.detected(t) || app.has_data(t) { Constraint::Fill(1) } else { Constraint::Length(3) })
        .collect();
    let areas = Layout::vertical(constraints).split(area);
    for (i, &t) in tools.iter().enumerate() {
        draw_panel(f, areas[i], app, t);
    }
}

fn draw_welcome(f: &mut Frame, area: Rect, app: &App) {
    let mut lines = vec![
        Line::from(""),
        Line::from(ratatui::text::Span::styled("No AI coding tool logs found yet.", theme::value_style())),
        Line::from(""),
        Line::from(dim("aitop reads the local logs written by:")),
    ];
    for st in &app.snap.status {
        let paths: Vec<String> = st.roots.iter().map(|(p, _)| tilde(p)).collect();
        lines.push(Line::from(vec![
            ratatui::text::Span::styled(
                format!("  {:<12}", st.tool.name()),
                Style::new().fg(theme::accent(st.tool)).bold(),
            ),
            dim(paths.join(", ")),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(dim("Use one of them, or point aitop elsewhere in the config file:")));
    lines.push(Line::from(dim(format!("  {}", tilde(app.config_path())))));
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(theme::border()));
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center).block(block), area);
}
