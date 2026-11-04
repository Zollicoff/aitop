//! Live activity feed: the newest responses across all tools.

use chrono::{Local, TimeZone};
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Row as TRow, Table};
use ratatui::Frame;

use super::widgets::{dim, val};
use crate::event::Event;
use crate::format;
use crate::theme;
use crate::ui::app::App;

/// Responses newer than this are highlighted.
const FRESH_MS: i64 = 10_000;
/// The title sums this much recent activity.
const RECENT_MS: i64 = 5 * 60_000;

pub(super) fn draw_feed(f: &mut Frame, area: Rect, app: &App) {
    let now = Local::now().timestamp_millis();
    let recent: Vec<&Event> = app.feed.iter().take_while(|e| now - e.ts <= RECENT_MS).collect();
    let recent_cost: f64 = recent.iter().map(|e| e.cost).sum();
    let title = Line::from(vec![
        Span::styled("┤ ", Style::new().fg(theme::border())),
        Span::styled("Activity", Style::new().fg(theme::bright()).bold()),
        dim(format!(
            " · last 5 min {} · {} response{} ",
            format::cost(recent_cost + 0.0),
            recent.len(),
            if recent.len() == 1 { "" } else { "s" }
        )),
        Span::styled("├", Style::new().fg(theme::border())),
    ]);
    let block = Block::bordered()
        .border_type(theme::border_type())
        .border_style(Style::new().fg(theme::border()))
        .title(title)
        .title(Line::from(dim(" a to hide ")).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    if app.feed.is_empty() {
        f.render_widget(Line::from(dim(" no responses yet")), inner);
        return;
    }

    let wide = inner.width >= 100;
    let mut widths = vec![
        Constraint::Length(8),  // time
        Constraint::Length(12), // tool
        Constraint::Fill(2),    // project
        Constraint::Fill(1),    // model
    ];
    if wide {
        widths.extend([Constraint::Length(7), Constraint::Length(7), Constraint::Length(8)]);
    }
    widths.extend([Constraint::Length(8), Constraint::Length(9)]);

    let rows = app.feed.iter().take(inner.height as usize).map(|e| {
        let fresh = now - e.ts <= FRESH_MS;
        let time =
            Local.timestamp_millis_opt(e.ts).single().map(|t| t.format("%H:%M:%S").to_string()).unwrap_or_default();
        let tool = Span::styled(e.tool.name(), Style::new().fg(theme::accent(e.tool)));
        let mut cells = vec![
            Cell::from(if fresh { Line::from(val(time)) } else { Line::from(dim(time)) }),
            Cell::from(Line::from(tool)),
            Cell::from(Line::from(val(e.project.to_string()))),
            Cell::from(Line::from(dim(format::short_model(&e.model)))),
        ];
        let right = |s: String| Cell::from(Line::from(s).right_aligned());
        if wide {
            cells.extend([
                right(format::tokens(e.tokens.input)).style(theme::dim_style()),
                right(format::tokens(e.tokens.output)).style(theme::dim_style()),
                right(format::tokens(e.tokens.cache_read + e.tokens.cache_writes())).style(theme::dim_style()),
            ]);
        }
        cells.push(right(format::tokens(e.tokens.total())));
        let cost = if e.priced { format::cost(e.cost) } else { "?".into() };
        cells.push(right(cost).style(Style::new().fg(if e.priced { theme::text() } else { theme::warn() }).bold()));
        let row = TRow::new(cells);
        if fresh {
            row.style(Style::new().add_modifier(Modifier::BOLD))
        } else {
            row
        }
    });
    let mut header = vec![Cell::from("Time"), Cell::from("Tool"), Cell::from("Project"), Cell::from("Model")];
    let right = |s: &'static str| Cell::from(Line::from(s).right_aligned());
    if wide {
        header.extend([right("In"), right("Out"), right("Cache")]);
    }
    header.extend([right("Tokens"), right("Cost")]);
    let table = Table::new(rows, widths)
        .header(TRow::new(header).style(Style::new().fg(theme::dim()).add_modifier(Modifier::BOLD)))
        .column_spacing(1);
    f.render_widget(table, inner);
}
