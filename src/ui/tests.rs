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
use crate::collector::Dataset;
use crate::event::{Interner, Tool};
use crate::sources::SourceStatus;
use crate::theme::{self, ThemeSet};

/// An app over a few events, with `theme` applied.
pub(super) fn app(theme_key: &str) -> App {
    let themes = ThemeSet::builtins();
    theme::set(themes.get(theme_key).unwrap().palette);
    let now = chrono::Local::now().timestamp_millis();
    let mut i = Interner::default();
    let events = vec![
        ev(&mut i, Tool::Claude, now - 3_600_000, "/code/webapp", "s1", 2.5),
        ev(&mut i, Tool::Claude, now - 60_000, "/code/api", "s2", 1.0),
        ev(&mut i, Tool::Codex, now - 30_000, "/code/api", "x1", 0.75),
    ];
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
        config_path: dir.join("config.toml"),
        themes,
        theme: theme_key.into(),
    };
    let mut app = App::new(Arc::new(RwLock::new(data)), tx, opts);
    app.refresh();
    app
}

pub(super) fn render(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| draw::draw(f, app)).unwrap();
    term.backend().buffer().clone()
}

pub(super) fn text(buf: &Buffer) -> String {
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
