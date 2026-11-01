//! Left half of a panel: totals, status, usage windows and the cost chart.
//!
//! Nothing here is tool-specific: a 5-hour window is shown when the summary
//! has one and rate limits when the tool reported them.

use chrono::{Local, TimeZone};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::chart::draw_chart;
use super::widgets::{dim, kv, meter, val};
use crate::aggregate::{Block, Range, ToolSummary};
use crate::event::Tool;
use crate::format;
use crate::limits::{LimitWindow, RateLimits};
use crate::theme;
use crate::ui::app::App;

/// Limits older than this get an "as of" note.
const STALE_LIMITS_MS: i64 = 15 * 60_000;

pub(super) fn draw_stats(f: &mut Frame, area: Rect, app: &App, tool: Tool, ts: &ToolSummary) {
    let Some(summary) = &app.summary else { return };
    let now = summary.now;
    let meter_w = (area.width as usize).saturating_sub(27).clamp(6, 24);

    let mut lines = totals(tool, ts, summary.range);
    lines.push(kv("Status", status(ts, now)));
    lines.extend(top_model(ts));
    if let Some(b) = &ts.block {
        lines.extend(block_lines(b, now, meter_w));
    }
    if let Some(Some(l)) = app.snap.limits.get(tool.index()) {
        lines.extend(limit_lines(l, now, meter_w));
    }

    let text_h = (lines.len() as u16).min(area.height);
    let [text_area, chart_area] = Layout::vertical([Constraint::Length(text_h), Constraint::Fill(1)]).areas(area);
    f.render_widget(Paragraph::new(lines), text_area);
    if chart_area.height >= 3 {
        draw_chart(f, chart_area, summary, tool, ts);
    }
}

/// Cost, tokens, cache and session counts.
fn totals(tool: Tool, ts: &ToolSummary, range: Range) -> Vec<Line<'static>> {
    let t = ts.tokens;
    let mut cost = vec![Span::styled(format::cost(ts.cost), Style::new().fg(theme::accent(tool)).bold())];
    if !ts.unpriced_models.is_empty() {
        cost.push(Span::styled(" + unpriced", Style::new().fg(theme::warn())));
    }
    cost.push(dim(format!("  {}", range.label())));

    let mut lines = vec![
        kv("Cost", cost),
        kv(
            "Tokens",
            vec![
                Span::styled(format::tokens(t.total()), theme::value_style()),
                dim("  in "),
                val(format::tokens(t.input)),
                dim(" out "),
                val(format::tokens(t.output)),
            ],
        ),
    ];
    if t.cache_read + t.cache_writes() > 0 {
        let mut c = vec![dim("read "), val(format::tokens(t.cache_read))];
        if t.cache_writes() > 0 {
            c.extend([dim(" write "), val(format::tokens(t.cache_writes()))]);
        }
        let hit = t.cache_read as f64 / t.prompt().max(1) as f64;
        c.push(dim(format!("  {:.0}% hit", hit * 100.0)));
        lines.push(kv("Cache", c));
    }
    lines.push(kv(
        "Sessions",
        vec![
            val(format::thousands(ts.sessions as u64)),
            dim(" · "),
            val(format::thousands(ts.messages)),
            dim(" responses"),
        ],
    ));
    lines
}

fn status(ts: &ToolSummary, now: i64) -> Vec<Span<'static>> {
    match ts.last_ts {
        Some(last) if ts.active_sessions > 0 => vec![
            Span::styled("● ", Style::new().fg(theme::good())),
            Span::styled(format!("{} active", ts.active_sessions), Style::new().fg(theme::good()).bold()),
            dim(format!(" · {} ago", format::duration(now - last))),
        ],
        Some(last) => vec![dim(format!("idle · last used {} ago", format::duration(now - last)))],
        None => vec![dim("no usage recorded yet")],
    }
}

/// The model with the highest cost (or tokens, if nothing is priced).
fn top_model(ts: &ToolSummary) -> Option<Line<'static>> {
    let top = ts.models.iter().max_by(|a, b| a.cost.total_cmp(&b.cost))?;
    let share =
        if ts.cost > 0.0 { top.cost / ts.cost } else { top.tokens.total() as f64 / ts.tokens.total().max(1) as f64 };
    let mut v = vec![val(format::short_model(&top.name))];
    if ts.models.len() > 1 {
        v.push(dim(format!("  {:.0}% · {} models", share * 100.0, ts.models.len())));
    }
    Some(kv("Model", v))
}

fn block_lines(b: &Block, now: i64, meter_w: usize) -> Vec<Line<'static>> {
    if !b.active {
        return vec![kv(
            "5h block",
            vec![dim(format!("none active · last ended {} ago", format::duration(now - b.end)))],
        )];
    }
    let elapsed = (now - b.start) as f64 / (b.end - b.start) as f64;
    let mut bar = meter(elapsed, meter_w, false);
    bar.push(dim(format!(" {} left", format::duration(b.end - now))));
    vec![
        kv("5h block", bar),
        kv(
            "",
            vec![
                val(format::cost(b.cost)),
                dim(format!(" · {}/h · ", format::cost(b.burn_per_hour))),
                val(format!("→ {}", format::cost(b.projected))),
                dim(format!(" by {}", clock(b.end))),
            ],
        ),
    ]
}

fn limit_lines(l: &RateLimits, now: i64, meter_w: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = [(&l.primary, "5h limit"), (&l.secondary, "Weekly")]
        .into_iter()
        .filter_map(|(w, fallback)| w.as_ref().map(|w| limit_line(w, fallback, now, meter_w)))
        .collect();
    if now - l.ts > STALE_LIMITS_MS {
        lines.push(kv("", vec![dim(format!("limits as of {} ago", format::duration(now - l.ts)))]));
    }
    lines
}

fn limit_line(w: &LimitWindow, fallback: &str, now: i64, meter_w: usize) -> Line<'static> {
    let label = match w.window_minutes {
        Some(m) if m >= 7 * 24 * 60 - 60 => "Weekly".to_string(),
        Some(m) if m >= 24 * 60 => format!("{}d limit", (m + 720) / 1440),
        Some(m) if m >= 60 => format!("{}h limit", (m + 30) / 60),
        Some(m) => format!("{m}m limit"),
        None => fallback.to_string(),
    };
    // A window whose reset time has passed has been reset, whatever the snapshot said.
    let reset_in = w.resets_at.map(|r| r - now);
    let used = if reset_in.is_some_and(|d| d <= 0) { 0.0 } else { w.used_percent };
    let mut v = meter(used / 100.0, meter_w, true);
    v.push(Span::styled(format!(" {used:>3.0}%"), Style::new().fg(theme::text()).bold()));
    match reset_in {
        Some(d) if d > 0 => v.push(dim(format!(" · {}", format::duration(d)))),
        Some(_) => v.push(dim(" · reset")),
        None => {}
    }
    kv(&label, v)
}

fn clock(ts: i64) -> String {
    Local.timestamp_millis_opt(ts).single().map(|t| t.format("%H:%M").to_string()).unwrap_or_default()
}
