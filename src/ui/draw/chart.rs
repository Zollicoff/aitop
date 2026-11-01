//! Cost-over-time sparkline with a caption and a time axis.

use chrono::{Local, TimeZone};
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Paragraph, Sparkline};
use ratatui::Frame;

use super::widgets::dim;
use crate::aggregate::{Bucket, Summary, ToolSummary};
use crate::event::Tool;
use crate::format;
use crate::theme;

pub(super) fn draw_chart(f: &mut Frame, area: Rect, summary: &Summary, tool: Tool, ts: &ToolSummary) {
    let width = area.width.saturating_sub(2) as usize;
    if width < 4 || ts.series.is_empty() {
        return;
    }
    let with_axis = area.height >= 5;
    let [caption, chart, axis] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(if with_axis { 1 } else { 0 }),
    ])
    .areas(area);

    let peak = ts.series.iter().copied().fold(0.0, f64::max);
    let unit = summary.bucket.label();
    let text = format!(" cost / {unit} · {} · peak {}", summary.range.label(), format::cost(peak));
    f.render_widget(Paragraph::new(dim(text)), caption);

    // The most recent buckets that fit, stretched across the full width.
    let first = ts.series.len().saturating_sub(width);
    let vals = &ts.series[first..];
    let data: Vec<u64> = (0..width).map(|col| (vals[col * vals.len() / width] * 10_000.0).round() as u64).collect();
    let inner = |r: Rect| r.inner(Margin { horizontal: 1, vertical: 0 });
    f.render_widget(Sparkline::default().data(&data).style(Style::new().fg(theme::accent(tool))), inner(chart));

    if with_axis {
        let start = summary.buckets.get(first).map(|&t| axis_label(t, summary.bucket)).unwrap_or_default();
        let end = if summary.bucket == Bucket::Hour { "now" } else { "today" };
        f.render_widget(Paragraph::new(dim(start)), inner(axis));
        f.render_widget(Paragraph::new(dim(end)).alignment(Alignment::Right), inner(axis));
    }
}

fn axis_label(ts: i64, bucket: Bucket) -> String {
    let Some(t) = Local.timestamp_millis_opt(ts).single() else { return String::new() };
    match bucket {
        Bucket::Hour => t.format("%H:%M").to_string(),
        Bucket::Day | Bucket::Week => t.format("%b %-d").to_string(),
    }
}
