//! Help overlay.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use ratatui::Frame;

use crate::format;
use crate::paths::tilde;
use crate::theme;
use crate::ui::app::App;

pub(super) fn draw_help(f: &mut Frame, area: Rect, app: &App) {
    let w = area.width.saturating_sub(4).min(78);
    let h = area.height.saturating_sub(2).min(30);
    let popup = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    f.render_widget(Clear, popup);
    let dim = theme::dim_style();
    let key = |k: &str, d: &str| {
        Line::from(vec![
            Span::styled(format!("  {k:<14}"), Style::new().fg(theme::text()).bold()),
            Span::styled(d.to_string(), dim),
        ])
    };
    let head = |s: &str| Line::from(Span::styled(format!(" {s}"), Style::new().fg(theme::bright()).bold()));
    let mut lines = vec![
        head("Keys"),
        key("1 2 3 4  [ ]", "range: today · 7 days · 30 days · all time"),
        key("tab ⇧tab", "focus next / previous tool"),
        key("↑ ↓ j k", "select row (PgUp/PgDn, g/G for top/bottom)"),
        key("← → h l", "switch view: projects · models · sessions (p m S)"),
        key("enter", "drill into the selected row · esc / backspace to back out"),
        key("/", "search the table · enter keeps it · esc clears it"),
        key("s", "cycle sort: cost · tokens · recent · name"),
        key("z", "zoom the focused tool (shows details of the selected row)"),
        key("a", "show / hide the live activity feed"),
        key("t  T", "next / previous theme"),
        key("r", "rescan logs now"),
        key("q  ctrl-c", "quit"),
        Line::from(""),
        head("Data"),
    ];
    for st in &app.snap.status {
        for (p, exists) in &st.roots {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<12}", st.tool.name()), Style::new().fg(theme::accent(st.tool))),
                Span::styled(
                    if *exists { "✓ " } else { "✗ " },
                    Style::new().fg(if *exists { theme::good() } else { theme::dim() }),
                ),
                Span::styled(tilde(p), dim),
            ]));
        }
    }
    let hist = match &app.snap.history_path {
        Some(p) => tilde(p),
        None => "disabled".into(),
    };
    lines.push(Line::from(vec![
        Span::styled("  history     ", Style::new().fg(theme::text())),
        Span::styled(hist, dim),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  config      ", Style::new().fg(theme::text())),
        Span::styled(tilde(app.config_path()), dim),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  theme       ", Style::new().fg(theme::text())),
        Span::styled(format!("{} · t / T to change · `aitop themes` lists them", app.theme_name()), dim),
    ]));
    for w in app.theme_warnings() {
        lines.push(Line::from(Span::styled(format!("  ! theme {w}"), Style::new().fg(theme::warn()))));
    }
    lines.push(Line::from(vec![
        Span::styled("  records     ", Style::new().fg(theme::text())),
        Span::styled(
            format!(
                "{} · last scan {}",
                format::thousands(app.snap.events as u64),
                app.snap.last_scan.map(|d| format!("{}ms", d.as_millis())).unwrap_or_else(|| "pending".into())
            ),
            dim,
        ),
    ]));
    lines.push(Line::from(""));
    lines.push(head("Pricing"));
    lines.push(Line::from(Span::styled(format!("  {}", app.snap.pricing_sources.join(" + ")), dim)));
    let unpriced: Vec<String> =
        app.summary.iter().flat_map(|s| s.tools.iter()).flat_map(|t| t.unpriced_models.iter().cloned()).collect();
    if !unpriced.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  no price for: {}", unpriced.join(", ")),
            Style::new().fg(theme::warn()),
        )));
        lines.push(Line::from(Span::styled(
            "  run `aitop pricing update`, or add it under [pricing] in the config",
            dim,
        )));
    }
    lines.push(Line::from(Span::styled(
        "  Costs are estimates at API list prices, not what a subscription bills.",
        dim,
    )));
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::dim()))
        .title(Line::from(Span::styled(" aitop help ", Style::new().fg(theme::bright()).bold())))
        .title(Line::from(Span::styled(" esc to close ", dim)).right_aligned())
        .style(Style::new().bg(theme::background()));
    f.render_widget(Paragraph::new(lines).block(block), popup);
}
