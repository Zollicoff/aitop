//! Rendering.

use chrono::{Local, TimeZone};
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, Paragraph, Row as TRow, Scrollbar, ScrollbarOrientation, ScrollbarState, Sparkline,
    Table,
};
use ratatui::Frame;

use super::{App, Hit, View};
use crate::aggregate::{Range, Row, ToolSummary};
use crate::config::tilde;
use crate::event::Tool;
use crate::format;
use crate::sources::codex::LimitWindow;
use crate::theme;

pub fn draw(f: &mut Frame, app: &mut App) {
    app.hits.clear();
    let area = f.area();
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    draw_header(f, header, app);
    draw_body(f, body, app);
    draw_footer(f, footer, app);
    if app.help {
        draw_help(f, area, app);
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &mut App) {
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
            spans.push(Span::styled(format!(" {label} "), Style::new().fg(theme::dim())));
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
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(clock, Style::new().fg(theme::dim())))).alignment(Alignment::Right),
        area,
    );
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let key = |k: &str| Span::styled(k.to_string(), Style::new().fg(theme::text()).bold());
    let txt = |t: &str| Span::styled(t.to_string(), Style::new().fg(theme::dim()));
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
        (at.elapsed().as_secs() < 2).then(|| Span::styled(format!("{msg} "), Style::new().fg(theme::dim())))
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
                Style::new().fg(theme::warn()),
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

fn draw_body(f: &mut Frame, area: Rect, app: &mut App) {
    if app.summary.is_none() {
        return;
    }
    let tools = if app.zoom { vec![app.focus] } else { app.visible() };
    let nothing_anywhere = !tools.iter().any(|&t| app.detected(t) || app.has_data(t));
    if nothing_anywhere && app.snap.loaded {
        draw_welcome(f, area, app);
        return;
    }
    let constraints: Vec<Constraint> = tools
        .iter()
        .map(|&t| if app.detected(t) || app.has_data(t) { Constraint::Fill(1) } else { Constraint::Length(3) })
        .collect();
    let areas = Layout::vertical(constraints).split(area);
    for (i, &t) in tools.iter().enumerate() {
        draw_panel(f, areas[i], app, t);
    }
}

fn draw_welcome(f: &mut Frame, area: Rect, app: &App) {
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled("No AI coding tool logs found yet.", theme::value_style())),
        Line::from(""),
        Line::from(Span::styled("aitop reads the local logs written by:", theme::dim_style())),
    ];
    for st in &app.snap.status {
        let paths: Vec<String> = st.roots.iter().map(|(p, _)| tilde(p)).collect();
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<12}", st.tool.name()), Style::new().fg(theme::accent(st.tool)).bold()),
            Span::styled(paths.join(", "), theme::dim_style()),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Use one of them, or point aitop elsewhere in the config file:",
        theme::dim_style(),
    )));
    lines.push(Line::from(Span::styled(format!("  {}", tilde(app.config_path())), theme::dim_style())));
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(theme::border()));
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center).block(block), area);
}

fn panel_block(tool: Tool, ts: &ToolSummary, focused: bool, range: Range) -> Block<'static> {
    let accent = theme::accent(tool);
    let dot = if ts.active_sessions > 0 {
        Span::styled("●", Style::new().fg(theme::good()))
    } else {
        Span::styled("○", Style::new().fg(theme::dim()))
    };
    let title = Line::from(vec![
        Span::styled("┤ ", Style::new().fg(if focused { accent } else { theme::border() })),
        dot,
        Span::raw(" "),
        Span::styled(tool.name(), Style::new().fg(accent).bold()),
        Span::styled(" ├", Style::new().fg(if focused { accent } else { theme::border() })),
    ]);
    let right = if range == Range::Today || ts.today_messages == 0 {
        Line::default()
    } else {
        Line::from(vec![
            Span::styled(" today ", theme::dim_style()),
            Span::styled(format::cost(ts.today_cost), Style::new().fg(theme::text())),
            Span::raw(" "),
        ])
        .right_aligned()
    };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { accent } else { theme::border() }))
        .title(title)
        .title(right)
}

fn draw_panel(f: &mut Frame, area: Rect, app: &mut App, tool: Tool) {
    let summary = app.summary.as_ref().expect("checked by caller");
    let ts = summary.tool(tool).clone();
    let block = panel_block(tool, &ts, app.focus == tool, app.range);
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.hits.push((area, Hit::Panel(tool)));

    if !app.detected(tool) && !app.has_data(tool) {
        let st = &app.snap.status[tool.index()];
        let paths: Vec<String> = st.roots.iter().map(|(p, _)| tilde(p)).collect();
        let msg = format!(" not installed — no logs in {}", paths.join(", "));
        f.render_widget(Paragraph::new(Span::styled(msg, theme::dim_style())), inner);
        return;
    }
    if inner.width < 20 || inner.height < 1 {
        return;
    }
    let (left, right) = if inner.width >= 74 {
        let lw = (inner.width * 2 / 5).clamp(36, 50);
        let [l, _, r] =
            Layout::horizontal([Constraint::Length(lw), Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
        (l, Some(r))
    } else if inner.height >= 12 {
        let [l, r] = Layout::vertical([Constraint::Length(7), Constraint::Fill(1)]).areas(inner);
        (l, Some(r))
    } else {
        (inner, None)
    };
    draw_stats(f, left, app, tool, &ts);
    if let Some(r) = right {
        draw_table(f, r, app, tool, &ts);
    }
}

fn kv(label: &str, value: Vec<Span<'static>>) -> Line<'static> {
    let mut v = vec![Span::styled(format!(" {label:<9}"), theme::label_style())];
    v.extend(value);
    Line::from(v)
}

fn draw_stats(f: &mut Frame, area: Rect, app: &App, tool: Tool, ts: &ToolSummary) {
    let summary = app.summary.as_ref().unwrap();
    let now = summary.now;
    let accent = theme::accent(tool);
    let dim = theme::dim_style();
    let val = |s: String| Span::styled(s, Style::new().fg(theme::text()));
    let t = ts.tokens;

    let mut lines: Vec<Line> = Vec::new();
    let mut cost_spans = vec![Span::styled(format::cost(ts.cost), Style::new().fg(accent).bold())];
    if !ts.unpriced_models.is_empty() {
        cost_spans.push(Span::styled(" + unpriced", Style::new().fg(theme::warn())));
    }
    cost_spans.push(Span::styled(format!("  {}", summary.range.label()), dim));
    lines.push(kv("Cost", cost_spans));
    lines.push(kv(
        "Tokens",
        vec![
            Span::styled(format::tokens(t.total()), theme::value_style()),
            Span::styled("  in ", dim),
            val(format::tokens(t.input)),
            Span::styled(" out ", dim),
            val(format::tokens(t.output)),
        ],
    ));
    if t.cache_read + t.cache_writes() > 0 {
        let mut c = vec![Span::styled("read ", dim), val(format::tokens(t.cache_read))];
        if t.cache_writes() > 0 {
            c.push(Span::styled(" write ", dim));
            c.push(val(format::tokens(t.cache_writes())));
        }
        let hit = t.cache_read as f64 / t.prompt().max(1) as f64;
        c.push(Span::styled(format!("  {:.0}% hit", hit * 100.0), dim));
        lines.push(kv("Cache", c));
    }
    lines.push(kv(
        "Sessions",
        vec![
            val(format::thousands(ts.sessions as u64)),
            Span::styled(" · ", dim),
            val(format::thousands(ts.messages)),
            Span::styled(" responses", dim),
        ],
    ));
    let status = match ts.last_ts {
        Some(last) if ts.active_sessions > 0 => vec![
            Span::styled("● ", Style::new().fg(theme::good())),
            Span::styled(format!("{} active", ts.active_sessions), Style::new().fg(theme::good()).bold()),
            Span::styled(format!(" · {} ago", format::duration(now - last)), dim),
        ],
        Some(last) => vec![Span::styled(format!("idle · last used {} ago", format::duration(now - last)), dim)],
        None => vec![Span::styled("no usage recorded yet", dim)],
    };
    lines.push(kv("Status", status));
    if let Some(top) = ts.models.iter().max_by(|a, b| a.cost.partial_cmp(&b.cost).unwrap_or(std::cmp::Ordering::Equal))
    {
        let share =
            if ts.cost > 0.0 { top.cost / ts.cost } else { top.tokens.total() as f64 / t.total().max(1) as f64 };
        let mut v = vec![val(format::short_model(&top.name))];
        if ts.models.len() > 1 {
            v.push(Span::styled(format!("  {:.0}% · {} models", share * 100.0, ts.models.len()), dim));
        }
        lines.push(kv("Model", v));
    }

    // Usage windows.
    let meter_w = (area.width as usize).saturating_sub(11 + 16).clamp(6, 24);
    match tool {
        Tool::Claude => {
            if let Some(b) = &ts.block {
                if b.active {
                    let frac = (now - b.start) as f64 / (b.end - b.start) as f64;
                    let mut l = vec![];
                    l.extend(meter(frac, meter_w, false));
                    l.push(Span::styled(format!(" {} left", format::duration(b.end - now)), dim));
                    lines.push(kv("5h block", l));
                    lines.push(kv(
                        "",
                        vec![
                            val(format::cost(b.cost)),
                            Span::styled(format!(" · {}/h · ", format::cost(b.burn_per_hour)), dim),
                            Span::styled(format!("→ {}", format::cost(b.projected)), Style::new().fg(theme::text())),
                            Span::styled(format!(" by {}", clock(b.end)), dim),
                        ],
                    ));
                } else {
                    lines.push(kv(
                        "5h block",
                        vec![Span::styled(
                            format!("none active · last ended {} ago", format::duration(now - b.end)),
                            dim,
                        )],
                    ));
                }
            }
        }
        Tool::Codex => {
            if let Some(l) = &app.snap.limits {
                for (w, fallback) in [(&l.primary, "5h limit"), (&l.secondary, "Weekly")] {
                    if let Some(w) = w {
                        lines.push(limit_line(w, fallback, now, meter_w));
                    }
                }
                if now - l.ts > 15 * 60_000 {
                    lines.push(kv(
                        "",
                        vec![Span::styled(format!("limits as of {} ago", format::duration(now - l.ts)), dim)],
                    ));
                }
            }
        }
    }

    let text_h = (lines.len() as u16).min(area.height);
    let [text_area, chart_area] = Layout::vertical([Constraint::Length(text_h), Constraint::Fill(1)]).areas(area);
    f.render_widget(Paragraph::new(lines), text_area);
    if chart_area.height >= 3 {
        draw_chart(f, chart_area, app, tool, ts);
    }
}

fn limit_line(w: &LimitWindow, fallback: &str, now: i64, meter_w: usize) -> Line<'static> {
    let label = match w.window_minutes {
        Some(m) if m >= 7 * 24 * 60 - 60 => "Weekly".to_string(),
        Some(m) if m >= 24 * 60 => format!("{}d limit", (m + 720) / 1440),
        Some(m) if m >= 60 => format!("{}h limit", (m + 30) / 60),
        Some(m) => format!("{m}m limit"),
        None => fallback.to_string(),
    };
    let reset = w.resets_at.filter(|&r| r > now);
    let used = if w.resets_at.is_some() && reset.is_none() { 0.0 } else { w.used_percent };
    let mut v = meter(used / 100.0, meter_w, true);
    v.push(Span::styled(format!(" {used:>3.0}%"), Style::new().fg(theme::text()).bold()));
    match (reset, w.resets_at) {
        (Some(r), _) => v.push(Span::styled(format!(" · {}", format::duration(r - now)), theme::dim_style())),
        (None, Some(_)) => v.push(Span::styled(" · reset", theme::dim_style())),
        _ => {}
    }
    kv(&label, v)
}

fn clock(ts: i64) -> String {
    Local.timestamp_millis_opt(ts).single().map(|t| t.format("%H:%M").to_string()).unwrap_or_default()
}

/// btop-style meter: one `■` per cell, coloured along a gradient.
fn meter(frac: f64, width: usize, by_level: bool) -> Vec<Span<'static>> {
    let frac = frac.clamp(0.0, 1.0);
    let filled = (frac * width as f64).round() as usize;
    let mut v = Vec::with_capacity(width);
    for i in 0..width {
        let pos = (i as f64 + 0.5) / width as f64;
        if i < filled {
            let c = if by_level { theme::gradient(pos) } else { theme::gradient(pos * 0.6) };
            v.push(Span::styled("■", Style::new().fg(c)));
        } else {
            v.push(Span::styled("■", Style::new().fg(theme::faint())));
        }
    }
    v
}

fn draw_chart(f: &mut Frame, area: Rect, app: &App, tool: Tool, ts: &ToolSummary) {
    let summary = app.summary.as_ref().unwrap();
    let width = area.width.saturating_sub(2) as usize;
    if width < 4 {
        return;
    }
    let series = &ts.series;
    let peak = series.iter().cloned().fold(0.0, f64::max);
    let caption = format!(" cost / {} · {} · peak {}", summary.bucket_unit, summary.range.label(), format::cost(peak));
    let with_axis = area.height >= 5;
    let [cap, chart, axis] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(if with_axis { 1 } else { 0 }),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(Span::styled(caption, theme::dim_style())), cap);
    // Most recent buckets that fit, stretched across the full width.
    let first = series.len().saturating_sub(width);
    let vals = &series[first..];
    if vals.is_empty() {
        return;
    }
    let data: Vec<u64> = (0..width).map(|col| (vals[col * vals.len() / width] * 10_000.0).round() as u64).collect();
    let spark = Sparkline::default().data(&data).style(Style::new().fg(theme::accent(tool)));
    let inner = chart.inner(Margin { horizontal: 1, vertical: 0 });
    f.render_widget(spark, inner);
    if with_axis {
        let label = |ts: i64| -> String {
            let Some(t) = Local.timestamp_millis_opt(ts).single() else { return String::new() };
            match summary.bucket_unit {
                "hour" => t.format("%H:%M").to_string(),
                _ => t.format("%b %-d").to_string(),
            }
        };
        let start = summary.buckets.get(first).map(|&t| label(t)).unwrap_or_default();
        let end = if summary.bucket_unit == "hour" { "now".to_string() } else { "today".to_string() };
        let axis_area = axis.inner(Margin { horizontal: 1, vertical: 0 });
        f.render_widget(Paragraph::new(Span::styled(start, theme::dim_style())), axis_area);
        f.render_widget(Paragraph::new(Span::styled(end, theme::dim_style())).alignment(Alignment::Right), axis_area);
    }
}

fn draw_table(f: &mut Frame, area: Rect, app: &mut App, tool: Tool, ts: &ToolSummary) {
    if area.height < 2 {
        return;
    }
    let accent = theme::accent(tool);
    let view = app.views[tool.index()];
    let focused = app.focus == tool;
    let [tabs, rest] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);

    // Tabs.
    let mut spans = Vec::new();
    let mut x = tabs.x;
    for v in View::ALL {
        let label = format!(" {} ", v.label());
        let w = label.chars().count() as u16;
        app.hits.push((Rect::new(x, tabs.y, w, 1), Hit::Tab(tool, v)));
        x += w + 1;
        let style = if v == view {
            Style::new().fg(accent).bold().add_modifier(Modifier::UNDERLINED)
        } else {
            theme::dim_style()
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), tabs);
    f.render_widget(
        Paragraph::new(Span::styled(format!("by {} ↓ ", app.sort.label()), theme::dim_style()))
            .alignment(Alignment::Right),
        tabs,
    );

    let show_detail = app.zoom && rest.height > 6;
    let [table_area, detail_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(if show_detail { 2 } else { 0 })]).areas(rest);

    let rows: Vec<Row> = app.rows(tool).to_vec();
    if rows.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(format!("no usage {}", app.range.label()), theme::dim_style()))
                .alignment(Alignment::Center),
            table_area.inner(Margin { horizontal: 0, vertical: (table_area.height / 2).saturating_sub(1) }),
        );
        return;
    }
    let now = app.summary.as_ref().unwrap().now;
    let w = table_area.width;
    let show_share = w >= 64;
    let show_last = w >= 50;
    let show_count = w >= 42;
    let total_cost: f64 = ts.cost.max(1e-12);
    let total_tokens = ts.tokens.total().max(1) as f64;

    let (name_h, count_h) = match view {
        View::Projects => ("Project", "Sess"),
        View::Models => ("Model", "Resp"),
        View::Sessions => ("Session", "Resp"),
    };
    let mut header = vec![Cell::from(name_h)];
    let mut widths = vec![Constraint::Fill(1)];
    if show_count {
        header.push(Cell::from(Line::from(count_h).right_aligned()));
        widths.push(Constraint::Length(6));
    }
    header.push(Cell::from(Line::from("Tokens").right_aligned()));
    widths.push(Constraint::Length(8));
    header.push(Cell::from(Line::from("Cost").right_aligned()));
    widths.push(Constraint::Length(9));
    if show_share {
        header.push(Cell::from("Share"));
        widths.push(Constraint::Length(11));
    }
    if show_last {
        header.push(Cell::from(Line::from("Last").right_aligned()));
        widths.push(Constraint::Length(5));
    }

    let trows: Vec<TRow> = rows
        .iter()
        .map(|r| {
            let name: Line = match view {
                View::Projects => Line::from(Span::styled(r.name.clone(), Style::new().fg(theme::text()))),
                View::Models => Line::from(Span::styled(format::short_model(&r.name), Style::new().fg(theme::text()))),
                View::Sessions => Line::from(vec![
                    Span::styled(r.name.clone(), Style::new().fg(theme::text())),
                    Span::styled(format!(" {}", r.key.chars().take(8).collect::<String>()), theme::dim_style()),
                ]),
            };
            let mut cells = vec![Cell::from(name)];
            if show_count {
                let n = if view == View::Projects { r.sessions as u64 } else { r.messages };
                cells.push(Cell::from(Line::from(format::thousands(n)).right_aligned()).style(theme::dim_style()));
            }
            cells.push(Cell::from(Line::from(format::tokens(r.tokens.total())).right_aligned()));
            let cost = if r.unpriced && r.cost == 0.0 {
                Span::styled("?", Style::new().fg(theme::warn()))
            } else if r.unpriced {
                Span::styled(format!("{}*", format::cost(r.cost)), Style::new().fg(theme::warn()))
            } else {
                Span::styled(format::cost(r.cost), Style::new().fg(theme::text()).bold())
            };
            cells.push(Cell::from(Line::from(cost).right_aligned()));
            if show_share {
                let share = if ts.cost > 0.0 { r.cost / total_cost } else { r.tokens.total() as f64 / total_tokens };
                let mut s = share_bar(share, 6, theme::accent(tool));
                s.push(Span::styled(format!(" {:>3.0}%", share * 100.0), theme::dim_style()));
                cells.push(Cell::from(Line::from(s)));
            }
            if show_last {
                cells.push(
                    Cell::from(Line::from(format::ago(now - r.last_ts)).right_aligned()).style(theme::dim_style()),
                );
            }
            TRow::new(cells)
        })
        .collect();

    let highlight =
        if focused { Style::new().bg(theme::select_bg()).add_modifier(Modifier::BOLD) } else { Style::new() };
    let table = Table::new(trows, widths)
        .header(TRow::new(header).style(Style::new().fg(theme::dim()).add_modifier(Modifier::BOLD)))
        .row_highlight_style(highlight)
        .column_spacing(1);
    let st = &mut app.tables[tool.index()];
    f.render_stateful_widget(table, table_area, st);

    let visible = table_area.height.saturating_sub(1) as usize;
    if rows.len() > visible {
        let mut sb = ScrollbarState::new(rows.len().saturating_sub(visible)).position(st.offset());
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_style(Style::new().fg(theme::faint()))
                .thumb_style(Style::new().fg(theme::dim())),
            table_area.inner(Margin { horizontal: 0, vertical: 1 }),
            &mut sb,
        );
    }

    if show_detail {
        if let Some(r) = st.selected().and_then(|i| rows.get(i)) {
            let dim = theme::dim_style();
            let v = |s: String| Span::styled(s, Style::new().fg(theme::text()));
            let first = match view {
                View::Projects => vec![Span::styled(" path ", dim), v(tilde(std::path::Path::new(&r.detail)))],
                View::Models => vec![Span::styled(" model ", dim), v(r.name.clone())],
                View::Sessions => vec![
                    Span::styled(" session ", dim),
                    v(r.key.clone()),
                    Span::styled("  model ", dim),
                    v(format::short_model(&r.detail)),
                ],
            };
            let t = r.tokens;
            let mut second = vec![
                Span::styled(" in ", dim),
                v(format::tokens(t.input)),
                Span::styled("  out ", dim),
                v(format::tokens(t.output)),
                Span::styled("  cache read ", dim),
                v(format::tokens(t.cache_read)),
            ];
            if t.cache_writes() > 0 {
                second.push(Span::styled("  write ", dim));
                second.push(v(format::tokens(t.cache_writes())));
            }
            second.push(Span::styled(
                format!(
                    "  · first {} ago · {} responses",
                    format::duration(now - r.first_ts),
                    format::thousands(r.messages)
                ),
                dim,
            ));
            f.render_widget(Paragraph::new(vec![Line::from(first), Line::from(second)]), detail_area);
        }
    }
}

fn share_bar(frac: f64, width: usize, color: ratatui::style::Color) -> Vec<Span<'static>> {
    let cells = frac.clamp(0.0, 1.0) * width as f64;
    let full = cells.floor() as usize;
    let mut v = vec![Span::styled("■".repeat(full), Style::new().fg(color))];
    let rest = width - full.min(width);
    if rest > 0 {
        v.push(Span::styled("■".repeat(rest), Style::new().fg(theme::faint())));
    }
    v
}

fn draw_help(f: &mut Frame, area: Rect, app: &App) {
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
        key("s", "cycle sort: cost · tokens · recent · name"),
        key("z  enter", "zoom the focused tool (shows details of the selected row)"),
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
        .title(Line::from(Span::styled(" esc to close ", dim)).right_aligned());
    f.render_widget(Paragraph::new(lines).block(block), popup);
}
