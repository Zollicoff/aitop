//! One bordered panel per tool: stats on the left, table on the right.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use super::stats::draw_stats;
use super::table::draw_table;
use super::widgets::{dim, val};
use crate::aggregate::{Range, ToolSummary};
use crate::event::Tool;
use crate::format;
use crate::paths::tilde;
use crate::theme;
use crate::ui::app::{App, Hit};

pub(super) fn draw_panel(f: &mut Frame, area: Rect, app: &mut App, tool: Tool) {
    let Some(summary) = app.summary.clone() else { return };
    let ts = summary.tool(tool);
    let crumbs: Vec<String> = app.panels[tool.index()].crumbs.iter().map(|(f, _)| f.label.clone()).collect();
    let block = panel_block(tool, ts, app.focus == tool, app.range, &crumbs);
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.hits.push((area, Hit::Panel(tool)));

    if !app.detected(tool) && !app.has_data(tool) {
        let roots: Vec<String> = app.snap.status[tool.index()].roots.iter().map(|(p, _)| tilde(p)).collect();
        f.render_widget(Paragraph::new(dim(format!(" not installed — no logs in {}", roots.join(", ")))), inner);
        return;
    }
    if inner.width < 20 || inner.height < 1 {
        return;
    }
    let (left, right) = split(inner);
    draw_stats(f, left, app, tool, ts);
    if let Some(r) = right {
        draw_table(f, r, app, tool, &summary);
    }
}

/// Side by side when wide, stacked when tall, stats only when cramped.
fn split(inner: Rect) -> (Rect, Option<Rect>) {
    if inner.width >= 74 {
        let lw = (inner.width * 2 / 5).clamp(36, 50);
        let [l, _, r] =
            Layout::horizontal([Constraint::Length(lw), Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
        (l, Some(r))
    } else if inner.height >= 12 {
        let [l, r] = Layout::vertical([Constraint::Length(7), Constraint::Fill(1)]).areas(inner);
        (l, Some(r))
    } else {
        (inner, None)
    }
}

/// The border, with `● Tool › crumb › crumb` as the title.
fn panel_block(tool: Tool, ts: &ToolSummary, focused: bool, range: Range, crumbs: &[String]) -> Block<'static> {
    let accent = theme::accent(tool);
    let edge = Style::new().fg(if focused { accent } else { theme::border() });
    let dot = if ts.active_sessions > 0 { Span::styled("●", Style::new().fg(theme::good())) } else { dim("○") };
    let mut title =
        vec![Span::styled("┤ ", edge), dot, Span::raw(" "), Span::styled(tool.name(), Style::new().fg(accent).bold())];
    for c in crumbs {
        title.push(dim(" › "));
        title.push(Span::styled(c.clone(), Style::new().fg(theme::bright()).bold()));
    }
    if !crumbs.is_empty() {
        title.push(dim("  esc ↩"));
    }
    title.push(Span::styled(" ├", edge));
    let title = Line::from(title);
    // Today's spend in the corner, unless the panel already shows today.
    let today = if range == Range::Today || ts.today_messages == 0 {
        Line::default()
    } else {
        Line::from(vec![dim(" today "), val(format::cost(ts.today_cost)), Span::raw(" ")]).right_aligned()
    };
    Block::bordered().border_type(theme::border_type()).border_style(edge).title(title).title(today)
}
