//! Small building blocks shared by the panels.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::budget::Level;
use crate::theme;

/// A dimmed span.
pub(super) fn dim(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), theme::dim_style())
}

/// A normal-weight value span.
pub(super) fn val(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), Style::new().fg(theme::text()))
}

/// A `label   value…` line with a fixed-width label column.
pub(super) fn kv(label: &str, value: Vec<Span<'static>>) -> Line<'static> {
    let mut v = vec![Span::styled(format!(" {label:<9}"), theme::label_style())];
    v.extend(value);
    Line::from(v)
}

/// btop-style meter: one `■` per cell, coloured along a green → red
/// gradient. With `by_level` false the gradient stops at yellow, for meters
/// (like elapsed time) where full is not alarming.
pub(super) fn meter(frac: f64, width: usize, by_level: bool) -> Vec<Span<'static>> {
    let filled = (frac.clamp(0.0, 1.0) * width as f64).round() as usize;
    (0..width)
        .map(|i| {
            let pos = (i as f64 + 0.5) / width as f64;
            let color = match (i < filled, by_level) {
                (false, _) => theme::faint(),
                (true, true) => theme::gradient(pos),
                (true, false) => theme::gradient(pos * 0.6),
            };
            Span::styled("■", Style::new().fg(color))
        })
        .collect()
}

/// Single-colour proportion bar.
pub(super) fn share_bar(frac: f64, width: usize, color: Color) -> Vec<Span<'static>> {
    let full = ((frac.clamp(0.0, 1.0) * width as f64).floor() as usize).min(width);
    vec![
        Span::styled("■".repeat(full), Style::new().fg(color)),
        Span::styled("■".repeat(width - full), Style::new().fg(theme::faint())),
    ]
}

/// Colour for a budget level.
pub(super) fn level_color(level: Level) -> Color {
    match level {
        Level::Ok => theme::good(),
        Level::Warn => theme::warn(),
        Level::Over => theme::bad(),
    }
}
