//! Rendering tests: draw frames into ratatui's in-memory backend.

use std::sync::mpsc;
use std::sync::{Arc, RwLock};

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

use super::app::App;
use super::{draw, Options};
use crate::aggregate::testutil::ev;
use crate::aggregate::Range;
use crate::budget::{BudgetConfig, Level};
use crate::collector::{Dataset, Shared};
use crate::event::{Interner, Tool};
use crate::sources::SourceStatus;
use crate::theme::{self, ThemeSet};

/// An app over a few events, with `theme` applied.
pub(crate) fn app(theme_key: &str) -> App {
    app_with(theme_key, BudgetConfig::default()).0
}

/// Like [`app`], with budgets, also returning the shared data so tests can
/// add events.
pub(crate) fn app_with(theme_key: &str, budget: BudgetConfig) -> (App, Shared) {
    let themes = ThemeSet::builtins();
    theme::set(themes.get(theme_key).unwrap().palette);
    let now = chrono::Local::now().timestamp_millis();
    // Keep every event "today", even just after midnight.
    let today = Range::Today.start(chrono::Local::now()).unwrap();
    let ago = |ms: i64| (now - ms).max(today);
    let mut i = Interner::default();
    let mut events = vec![
        ev(&mut i, Tool::Claude, ago(3_600_000), "/code/webapp", "s1", 2.5),
        ev(&mut i, Tool::Claude, ago(60_000), "/code/api", "s2", 1.0),
        ev(&mut i, Tool::Codex, ago(30_000), "/code/api", "x1", 0.75),
    ];
    events.sort_by_key(|e| e.ts);
    let dir = std::env::temp_dir();
    let data = Dataset {
        events,
        status: Tool::ALL.iter().map(|&t| SourceStatus::new(t, std::slice::from_ref(&dir))).collect(),
        limits: vec![None; Tool::ALL.len()],
        version: 1,
        loaded: true,
        ..Default::default()
    };
    let (tx, _rx) = mpsc::channel();
    let cfg = crate::config::Config { budget, ..Default::default() };
    let opts =
        Options { range: Range::Week, cfg, config_path: dir.join("config.toml"), themes, theme: theme_key.into() };
    let shared: Shared = Arc::new(RwLock::new(data));
    let mut app = App::new(shared.clone(), tx, opts);
    app.refresh();
    (app, shared)
}

pub(crate) fn render(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| draw::draw(f, app)).unwrap();
    term.backend().buffer().clone()
}

pub(crate) fn text(buf: &Buffer) -> String {
    let w = buf.area.width as usize;
    buf.content.chunks(w).map(|row| row.iter().map(|c| c.symbol()).collect::<String>() + "\n").collect()
}

#[test]
fn renders_both_tools() {
    let mut a = app("aitop");
    let out = text(&render(&mut a, 120, 36));
    assert!(out.contains("Claude Code") && out.contains("Codex"), "{out}");
    assert!(out.contains("webapp") && out.contains("$3.50"), "{out}");
}

#[test]
fn theme_background_covers_every_cell() {
    let mut a = app("dracula");
    let bg = theme::background();
    let buf = render(&mut a, 120, 36);
    let selection = theme::select_bg();
    let wrong: Vec<(u16, u16)> = buf
        .content
        .iter()
        .enumerate()
        .filter(|(_, c)| c.bg != bg && c.bg != selection)
        .map(|(i, _)| ((i % 120) as u16, (i / 120) as u16))
        .take(10)
        .collect();
    assert!(wrong.is_empty(), "cells without the theme background: {wrong:?}");
}

#[test]
fn budgets_show_and_alert_on_change() {
    // $4.25 spent today against a $5 daily budget: nearly used, no alert at startup.
    let budget: BudgetConfig = toml::from_str("daily = 5.0\n[claude]\ndaily = 3.0").unwrap();
    let (mut a, data) = app_with("aitop", budget);
    assert!(a.flash.is_none(), "levels at startup are not alerts");
    let out = text(&render(&mut a, 130, 40));
    assert!(out.contains("Day $") && out.contains("$4.25/$5.00"), "{out}");
    assert!(out.contains("over Claude Code daily budget"), "claude is past $3: {out}");

    // More spending pushes the all-tools budget over.
    {
        let mut d = data.write().unwrap();
        let mut e = d.events.last().unwrap().clone();
        e.ts += 1;
        e.cost = 2.0;
        e.id = "new".into();
        d.events.push(e);
        d.version += 1;
    }
    a.refresh();
    let f = a.flash.as_ref().expect("alert");
    assert_eq!(f.level, Level::Over);
    assert!(f.text.contains("daily budget used up"), "{}", f.text);
}

#[test]
fn activity_feed_lists_newest_first() {
    let mut a = app("aitop");
    assert!(!text(&render(&mut a, 120, 40)).contains("Activity"));
    a.on_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('a'),
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    let out = text(&render(&mut a, 120, 40));
    assert!(out.contains("Activity") && out.contains("last 5 min $"), "{out}");
    let feed: Vec<&str> = out.lines().filter(|l| l.contains(":") && l.contains("$")).collect();
    let codex = feed.iter().position(|l| l.contains("Codex") && l.contains("$0.75")).unwrap();
    let claude = feed.iter().position(|l| l.contains("Claude Code") && l.contains("$2.50")).unwrap();
    assert!(codex < claude, "newest first: {feed:?}");
}

#[test]
fn theme_background_can_be_turned_off() {
    let mut a = app("dracula");
    theme::set_look(theme::Look { theme_background: false, ..theme::look() });
    let buf = render(&mut a, 120, 36);
    let selection = theme::select_bg();
    assert!(
        buf.content.iter().all(|c| c.bg == ratatui::style::Color::Reset || c.bg == selection),
        "only the selection keeps a background"
    );
    theme::set_look(theme::Look::default());
}

#[test]
fn box_style_applies() {
    let mut a = app("aitop");
    theme::set_look(theme::Look { boxes: crate::appearance::BoxStyle::Double, ..theme::Look::default() });
    let out = text(&render(&mut a, 120, 36));
    theme::set_look(theme::Look::default());
    assert!(out.contains('╔') && !out.contains('╭'), "{out}");
}

#[test]
fn machines_tab_appears_with_a_second_machine() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
    let (mut a, data) = app_with("aitop", BudgetConfig::default());
    assert!(!text(&render(&mut a, 120, 36)).contains("Machines"), "one machine: no tab for it");
    a.on_key(key(KeyCode::Char('M')));
    assert_eq!(a.panels[Tool::Claude.index()].view, super::app::View::Projects);

    {
        let mut d = data.write().unwrap();
        let mut i = Interner::default();
        let mut e = ev(&mut i, Tool::Claude, d.events[0].ts + 1, "/code/webapp", "s9", 4.0);
        e.host = i.get("laptop");
        d.events.insert(1, e);
        d.host = "here".into();
        d.machines = 2;
        d.version += 1;
    }
    a.refresh();
    a.on_key(key(KeyCode::Char('M')));
    a.show_feed = true;
    let out = text(&render(&mut a, 120, 44));
    assert!(out.contains("Machines") && out.contains("here · this one"), "{out}");
    assert!(out.contains("laptop") && out.contains("Machine "), "the feed says where each response was made: {out}");
    assert!(out.contains("$7.50"), "the total covers both machines: {out}");

    // Help says how each remote is doing.
    data.write().unwrap().remotes = vec![crate::collector::RemoteStatus {
        name: "laptop".into(),
        target: "ssh laptop".into(),
        records: 1,
        error: Some("Connection timed out".into()),
        ..Default::default()
    }];
    a.refresh();
    a.help = true;
    let help = text(&render(&mut a, 120, 44));
    assert!(help.contains("here (this one)"), "{help}");
    assert!(help.contains("✗ laptop · 1 records · not synced: Connection timed out"), "{help}");
    a.help = false;

    // By cost the laptop ($4.00) is first; drilling in shows its projects.
    a.on_key(key(KeyCode::Enter));
    a.refresh();
    let p = &a.panels[Tool::Claude.index()];
    assert_eq!((p.view, p.crumbs[0].0.label.as_str()), (super::app::View::Projects, "laptop"));
    assert_eq!(a.summary.as_ref().unwrap().tool(Tool::Claude).cost, 4.0);
    // ← from the first tab wraps round to Machines.
    a.on_key(key(KeyCode::Left));
    assert_eq!(a.panels[Tool::Claude.index()].view, super::app::View::Machines);
}

#[test]
fn other_agents_share_a_panel_once_there_is_usage() {
    use crate::event::Rollup;
    let (mut a, data) = app_with("aitop", BudgetConfig::default());
    let out = text(&render(&mut a, 120, 40));
    assert!(!out.contains("Other agents"), "nothing to show yet: {out}");
    assert_eq!(a.visible(), [Tool::Claude, Tool::Codex]);

    {
        let mut d = data.write().unwrap();
        let mut i = Interner::default();
        let mut e = ev(&mut i, Tool::Other, d.events[0].ts, "t3-agent://grok", "t3:here", 0.2);
        e.project = i.get("Grok Build");
        e.model = i.get("grok-build");
        e.rollup = Some(Rollup { responses: 7, sessions: 3, cost: Some(0.2), unpriced: false });
        d.events.insert(0, e);
        d.version += 1;
    }
    a.refresh();
    assert_eq!(a.visible(), [Tool::Claude, Tool::Codex, Tool::Other]);
    let out = text(&render(&mut a, 120, 44));
    assert!(out.contains("Other agents") && out.contains("Agents") && out.contains("Grok Build"), "{out}");
    assert!(out.contains("$4.45"), "the header total includes it: {out}");
    // It is the smallest panel, and can be focused and zoomed like the others.
    a.focus = Tool::Other;
    a.zoom = true;
    let zoomed = text(&render(&mut a, 120, 30));
    assert!(zoomed.contains("totals from T3 Code") && zoomed.contains("Agent "), "{zoomed}");
    // No Sessions tab here (T3 Code does not say which sessions), so an agent opens on its models.
    assert!(zoomed.contains("Agents   Models") && !zoomed.contains("Models   Sessions"), "{zoomed}");
    a.on_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Enter,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(a.panels[Tool::Other.index()].view, super::app::View::Models);
}
