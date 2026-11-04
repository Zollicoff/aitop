//! The per-tool chart: cost or tokens over time, with a caption and axis.

use chrono::{Local, TimeZone};
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::graph::Graph;
use super::widgets::dim;
use crate::aggregate::{Bucket, Summary, ToolSummary};
use crate::appearance::{ChartMetric, GraphSymbol};
use crate::event::Tool;
use crate::format;
use crate::theme;
use crate::ui::app::App;

pub(super) fn draw_chart(f: &mut Frame, area: Rect, app: &App, summary: &Summary, tool: Tool, ts: &ToolSummary) {
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

    let cfg = app.cfg();
    let tokens: Vec<f64>;
    let (values, metric, peak) = match cfg.chart {
        ChartMetric::Cost => {
            let peak = ts.series.iter().copied().fold(0.0, f64::max);
            (&ts.series, "cost", format::cost(peak))
        }
        ChartMetric::Tokens => {
            tokens = ts.series_tokens.iter().map(|&t| t as f64).collect();
            let peak = ts.series_tokens.iter().copied().max().unwrap_or(0);
            (&tokens, "tokens", format::tokens(peak))
        }
    };
    let unit = summary.bucket.label();
    let text = format!(" {metric} / {unit} · {} · peak {peak}", summary.range.label());
    f.render_widget(Paragraph::new(dim(text)), caption);

    let inner = |r: Rect| r.inner(Margin { horizontal: 1, vertical: 0 });
    let graph = Graph { values, symbol: cfg.graph_symbol, color: cfg.graph_color, accent: theme::accent(tool) };
    f.render_widget(graph, inner(chart));

    if with_axis {
        // Braille fits two buckets per cell.
        let per_cell = if cfg.graph_symbol == GraphSymbol::Braille { 2 } else { 1 };
        let first = values.len().saturating_sub(width * per_cell);
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
