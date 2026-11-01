//! Right half of a panel: view tabs and the projects / models / sessions table.

use std::path::Path;

use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row as TRow, Scrollbar, ScrollbarOrientation, ScrollbarState, Table};
use ratatui::Frame;

use super::widgets::{dim, share_bar, val};
use crate::aggregate::{Row, Summary, ToolSummary};
use crate::event::Tool;
use crate::format;
use crate::paths::tilde;
use crate::theme;
use crate::ui::app::{rows_for, App, Hit, View};

pub(super) fn draw_table(f: &mut Frame, area: Rect, app: &mut App, tool: Tool, summary: &Summary) {
    if area.height < 2 {
        return;
    }
    let ts = summary.tool(tool);
    let view = app.panels[tool.index()].view;
    let rows = rows_for(ts, view);
    let [tabs, rest] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    draw_tabs(f, tabs, app, tool);

    let show_detail = app.zoom && rest.height > 6;
    let [table_area, detail_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(if show_detail { 2 } else { 0 })]).areas(rest);
    if rows.is_empty() {
        let middle = table_area.inner(Margin { horizontal: 0, vertical: (table_area.height / 2).saturating_sub(1) });
        f.render_widget(
            Paragraph::new(dim(format!("no usage {}", app.range.label()))).alignment(Alignment::Center),
            middle,
        );
        return;
    }

    // Leave the last column free for the scrollbar when the rows don't fit.
    let visible = table_area.height.saturating_sub(1) as usize;
    let scrolls = rows.len() > visible;
    let [rows_area, bar_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(if scrolls { 1 } else { 0 })]).areas(table_area);
    let columns = Column::fitting(rows_area.width);
    let ctx = CellCtx { ts, view, tool, now: summary.now };
    let header = TRow::new(columns.iter().map(|c| c.header(view))).style(Style::new().fg(theme::dim()).bold());
    let body = rows.iter().map(|r| TRow::new(columns.iter().map(|c| c.cell(r, &ctx))));
    let highlight =
        if app.focus == tool { Style::new().bg(theme::select_bg()).add_modifier(Modifier::BOLD) } else { Style::new() };
    let table = Table::new(body, columns.iter().map(|c| c.width()))
        .header(header)
        .row_highlight_style(highlight)
        .column_spacing(1);
    let state = &mut app.panels[tool.index()].table;
    f.render_stateful_widget(table, rows_area, state);

    if scrolls {
        draw_scrollbar(f, bar_area, rows.len() - visible, state.offset());
    }
    if show_detail {
        if let Some(r) = state.selected().and_then(|i| rows.get(i)) {
            draw_detail(f, detail_area, r, view, summary.now);
        }
    }
}

fn draw_tabs(f: &mut Frame, area: Rect, app: &mut App, tool: Tool) {
    let current = app.panels[tool.index()].view;
    let mut spans = Vec::new();
    let mut x = area.x;
    for v in View::ALL {
        let label = format!(" {} ", v.label());
        let w = label.chars().count() as u16;
        app.hits.push((Rect::new(x, area.y, w, 1), Hit::Tab(tool, v)));
        x += w + 1;
        let style = if v == current {
            Style::new().fg(theme::accent(tool)).bold().add_modifier(Modifier::UNDERLINED)
        } else {
            theme::dim_style()
        };
        spans.extend([Span::styled(label, style), Span::raw(" ")]);
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    f.render_widget(Paragraph::new(dim(format!("by {} ↓ ", app.sort.label()))).alignment(Alignment::Right), area);
}

fn draw_scrollbar(f: &mut Frame, area: Rect, max_offset: usize, offset: usize) {
    let mut state = ScrollbarState::new(max_offset).position(offset);
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_style(Style::new().fg(theme::faint()))
        .thumb_style(Style::new().fg(theme::dim()));
    f.render_stateful_widget(bar, area.inner(Margin { horizontal: 0, vertical: 1 }), &mut state);
}

/// Two lines describing the selected row (zoom mode).
fn draw_detail(f: &mut Frame, area: Rect, r: &Row, view: View, now: i64) {
    let first = match view {
        View::Projects => vec![dim(" path "), val(tilde(Path::new(&r.detail)))],
        View::Models => vec![dim(" model "), val(r.name.clone())],
        View::Sessions => {
            vec![dim(" session "), val(r.key.clone()), dim("  model "), val(format::short_model(&r.detail))]
        }
    };
    let t = r.tokens;
    let mut second = vec![
        dim(" in "),
        val(format::tokens(t.input)),
        dim("  out "),
        val(format::tokens(t.output)),
        dim("  cache read "),
        val(format::tokens(t.cache_read)),
    ];
    if t.cache_writes() > 0 {
        second.extend([dim("  write "), val(format::tokens(t.cache_writes()))]);
    }
    second.push(dim(format!(
        "  · first {} ago · {} responses",
        format::duration(now - r.first_ts),
        format::thousands(r.messages)
    )));
    f.render_widget(Paragraph::new(vec![Line::from(first), Line::from(second)]), area);
}

struct CellCtx<'a> {
    ts: &'a ToolSummary,
    view: View,
    tool: Tool,
    now: i64,
}

#[derive(Clone, Copy)]
enum Column {
    Name,
    Count,
    Tokens,
    Cost,
    Share,
    Last,
}

impl Column {
    /// Columns that fit in `width`, dropping the least important first.
    fn fitting(width: u16) -> Vec<Column> {
        let mut v = vec![Column::Name];
        if width >= 42 {
            v.push(Column::Count);
        }
        v.extend([Column::Tokens, Column::Cost]);
        if width >= 64 {
            v.push(Column::Share);
        }
        if width >= 50 {
            v.push(Column::Last);
        }
        v
    }

    fn width(self) -> Constraint {
        match self {
            Column::Name => Constraint::Fill(1),
            Column::Count => Constraint::Length(6),
            Column::Tokens => Constraint::Length(8),
            Column::Cost => Constraint::Length(9),
            Column::Share => Constraint::Length(11),
            Column::Last => Constraint::Length(5),
        }
    }

    fn header(self, view: View) -> Cell<'static> {
        let right = |s: &'static str| Cell::from(Line::from(s).right_aligned());
        match self {
            Column::Name => Cell::from(match view {
                View::Projects => "Project",
                View::Models => "Model",
                View::Sessions => "Session",
            }),
            Column::Count => right(if view == View::Projects { "Sess" } else { "Resp" }),
            Column::Tokens => right("Tokens"),
            Column::Cost => right("Cost"),
            Column::Share => Cell::from("Share"),
            Column::Last => right("Last"),
        }
    }

    fn cell(self, r: &Row, ctx: &CellCtx) -> Cell<'static> {
        let right = |l: Line<'static>| Cell::from(l.right_aligned());
        match self {
            Column::Name => Cell::from(match ctx.view {
                View::Projects => Line::from(val(r.name.clone())),
                View::Models => Line::from(val(format::short_model(&r.name))),
                View::Sessions => Line::from(vec![
                    val(r.name.clone()),
                    dim(format!(" {}", r.key.chars().take(8).collect::<String>())),
                ]),
            }),
            Column::Count => {
                let n = if ctx.view == View::Projects { r.sessions as u64 } else { r.messages };
                right(Line::from(format::thousands(n))).style(theme::dim_style())
            }
            Column::Tokens => right(Line::from(format::tokens(r.tokens.total()))),
            Column::Cost => right(Line::from(cost_span(r))),
            Column::Share => {
                let share = if ctx.ts.cost > 0.0 {
                    r.cost / ctx.ts.cost
                } else {
                    r.tokens.total() as f64 / ctx.ts.tokens.total().max(1) as f64
                };
                let mut s = share_bar(share, 6, theme::accent(ctx.tool));
                s.push(dim(format!(" {:>3.0}%", share * 100.0)));
                Cell::from(Line::from(s))
            }
            Column::Last => right(Line::from(format::ago(ctx.now - r.last_ts))).style(theme::dim_style()),
        }
    }
}

/// Cost, with `?` when nothing in the row is priced and `*` when part is.
fn cost_span(r: &Row) -> Span<'static> {
    let warn = Style::new().fg(theme::warn());
    match (r.unpriced, r.cost > 0.0) {
        (true, false) => Span::styled("?", warn),
        (true, true) => Span::styled(format!("{}*", format::cost(r.cost)), warn),
        (false, _) => Span::styled(format::cost(r.cost), Style::new().fg(theme::text()).bold()),
    }
}
