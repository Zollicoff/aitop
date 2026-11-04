//! Bottom line: key hints and status messages.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::widgets::dim;
use crate::format;
use crate::theme;
use crate::ui::app::App;

pub(super) fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let key = |k: &str| Span::styled(k.to_string(), Style::new().fg(theme::text()).bold());
    let txt = |t: &str| dim(t.to_string());
    let mut spans = vec![txt(" ")];
    for (k, t) in [
        ("q", " quit  "),
        ("tab", " panel  "),
        ("↑↓", " select  "),
        ("←→", " view  "),
        ("1-4", " range  "),
        ("s", " sort  "),
        ("z", " zoom  "),
        ("?", " help"),
    ] {
        if area.width < 70 && matches!(k, "s" | "z" | "1-4") {
            continue;
        }
        spans.push(key(k));
        spans.push(txt(t));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);

    let status: Option<Span> = if let Some((msg, at)) = &app.flash {
        (at.elapsed().as_secs() < 3).then(|| dim(format!("{msg} ")))
    } else {
        None
    };
    let status = status.or_else(|| {
        if !app.snap.loaded {
            return Some(Span::styled("scanning logs… ", Style::new().fg(theme::warn())));
        }
        if let Some(e) = &app.snap.history_error {
            return Some(Span::styled(
                format!("⚠ history: {} ", format::truncate(e, 40)),
                Style::new().fg(theme::bad()),
            ));
        }
        let unpriced: Vec<&str> = app
            .summary
            .iter()
            .flat_map(|s| s.tools.iter())
            .flat_map(|t| t.unpriced_models.iter().map(String::as_str))
            .filter(|m| *m != "unknown")
            .collect();
        if !unpriced.is_empty() {
            let list = format::truncate(&unpriced.join(", "), 32);
            return Some(Span::styled(format!("⚠ no price for {list} — see ? "), Style::new().fg(theme::warn())));
        }
        None
    });
    if let Some(s) = status {
        if area.width > 90 {
            f.render_widget(Paragraph::new(Line::from(s)).alignment(Alignment::Right), area);
        }
    }
}
