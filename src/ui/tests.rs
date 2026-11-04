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
    let opts = Options {
        range: Range::Week,
        active_window_ms: 5 * 60_000,
        hide_missing: false,
        show_feed: false,
        config_path: dir.join("config.toml"),
        themes,
        theme: theme_key.into(),
        budget,
    };
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
