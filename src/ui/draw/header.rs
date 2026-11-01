//! Top line: ranges, totals, clock.

use chrono::Local;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::widgets::dim;
use crate::aggregate::Range;
use crate::format;
use crate::theme;
use crate::ui::app::{App, Hit};

pub(super) fn draw_header(f: &mut Frame, area: Rect, app: &mut App) {
    let mut spans = vec![
        Span::styled(" aitop ", Style::new().fg(theme::bright()).add_modifier(Modifier::BOLD)),
        Span::styled("│ ", Style::new().fg(theme::border())),
    ];
    let mut x = area.x + 9;
    for (i, r) in Range::ALL.iter().enumerate() {
        let label = format!("{} {}", i + 1, r.short());
        let w = label.chars().count() as u16 + 2;
        app.hits.push((Rect::new(x, area.y, w, 1), Hit::Range(*r)));
        x += w;
        if *r == app.range {
            spans.push(Span::styled(
                format!(" {label} "),
                Style::new().fg(theme::bright()).bg(theme::select_bg()).bold(),
            ));
        } else {
            spans.push(dim(format!(" {label} ")));
        }
    }
    if let Some(s) = &app.summary {
        spans.push(Span::styled(" │ ", Style::new().fg(theme::border())));
        spans.push(Span::styled(format::cost(s.total_cost), theme::value_style()));
        spans
            .push(Span::styled(format!(" · {} tokens", format::tokens(s.total_tokens)), Style::new().fg(theme::dim())));
        if s.active_sessions > 0 {
            spans.push(Span::styled(" │ ", Style::new().fg(theme::border())));
            spans.push(Span::styled("● ", Style::new().fg(theme::good())));
            spans.push(Span::styled(format!("{} active", s.active_sessions), Style::new().fg(theme::text())));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    let clock = Local::now().format("%H:%M:%S ").to_string();
    f.render_widget(Paragraph::new(Line::from(dim(clock))).alignment(Alignment::Right), area);
}
