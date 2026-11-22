//! The agent list of the list layout: every shown agent with its cost, the
//! focused one highlighted. The panel beside it shows that agent in full.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use super::widgets::dim;
use crate::event::Tool;
use crate::format;
use crate::theme;
use crate::ui::app::{App, Hit};

/// How wide the list is in a body `width` columns wide.
pub(super) fn width(width: u16) -> u16 {
    (width / 4).clamp(26, 34)
}

pub(super) fn draw_agents(f: &mut Frame, area: Rect, app: &mut App) {
    let Some(summary) = app.summary.clone() else { return };
    let edge = Style::new().fg(theme::border());
    let title = Line::from(vec![
        Span::styled("┤ ", edge),
        Span::styled("Agents", Style::new().fg(theme::bright()).bold()),
        Span::styled(" ├", edge),
    ]);
    let block = Block::bordered().border_type(theme::border_type()).border_style(edge).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let tools = app.visible();
    if inner.height == 0 || inner.width < 12 {
        return;
    }
    // Two lines an agent when there is room for all of them and the total.
    let total_rows = if inner.height as usize >= tools.len() + 3 { 2 } else { 0 };
    let per = if inner.height as usize >= tools.len() * 3 + total_rows as usize { 2 } else { 1 };
    let [list, total] = Layout::vertical([Constraint::Fill(1), Constraint::Length(total_rows)]).areas(inner);

    let w = inner.width as usize;
    let mut y = list.y;
    for &tool in &tools {
        if y + per > list.bottom() {
            break;
        }
        let ts = summary.tool(tool);
        let (cost, tokens, active) = summary.whole[tool.index()];
        let focused = app.focus == tool;
        let known = app.detected(tool) || app.has_data(tool);
        let dot = if active > 0 { Span::styled("●", Style::new().fg(theme::good())) } else { dim("○") };
        let amount = if !known {
            "not installed".to_string()
        } else if ts.unpriced_models.is_empty() || cost > 0.0 {
            format::cost(cost)
        } else {
            "?".to_string()
        };
        let name_w = w.saturating_sub(amount.chars().count() + 4);
        let name_style = if known { Style::new().fg(theme::accent(tool)).bold() } else { theme::dim_style() };
        let amount_style = if known { theme::value_style() } else { theme::dim_style() };
        let mut lines = vec![Line::from(vec![
            Span::raw(" "),
            dot,
            Span::raw(" "),
            Span::styled(format!("{:<name_w$}", format::truncate(tool.name(), name_w)), name_style),
            Span::styled(amount, amount_style),
            Span::raw(" "),
        ])];
        if per == 2 {
            let detail = if known {
                format!("   {} tokens · {} sess", format::tokens(tokens), format::thousands(ts.sessions as u64))
            } else {
                String::new()
            };
            lines.push(Line::from(dim(format::truncate(&detail, w))));
        }
        let rect = Rect::new(list.x, y, list.width, per);
        let style =
            if focused { Style::new().bg(theme::select_bg()).add_modifier(Modifier::BOLD) } else { Style::new() };
        f.render_widget(Paragraph::new(lines).style(style), rect);
        app.hits.push((rect, Hit::Panel(tool)));
        // A blank line between agents when they have two lines each.
        y += per + u16::from(per == 2);
    }

    if total_rows > 0 {
        let (cost, tokens) = shown_total(app, &summary.whole);
        let amount = format::cost(cost);
        let label_w = w.saturating_sub(amount.chars().count() + 2);
        let lines = vec![
            Line::from(Span::styled("─".repeat(w), Style::new().fg(theme::faint()))),
            Line::from(vec![
                Span::styled(
                    format!(" {:<label_w$}", format!("Total · {} tok", format::tokens(tokens))),
                    theme::dim_style(),
                ),
                Span::styled(amount, theme::value_style()),
                Span::raw(" "),
            ]),
        ];
        f.render_widget(Paragraph::new(lines), total);
    }
}

/// Cost and tokens of the agents the dashboard shows.
pub(super) fn shown_total(app: &App, whole: &[(f64, u64, usize)]) -> (f64, u64) {
    Tool::ALL
        .into_iter()
        .filter(|t| app.shown(*t))
        .fold((0.0, 0), |(c, n), t| (c + whole[t.index()].0, n + whole[t.index()].1))
}
