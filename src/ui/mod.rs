//! Interactive dashboard.

mod app;
mod draw;
mod input;
#[cfg(test)]
mod tests;

use std::io::stdout;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event as TEvent};
use ratatui::crossterm::execute;

use crate::aggregate::Range;
use crate::collector::{Command, Shared};
use crate::theme::ThemeSet;
use app::App;

pub struct Options {
    pub range: Range,
    pub active_window_ms: i64,
    pub hide_missing: bool,
    pub config_path: PathBuf,
    pub themes: ThemeSet,
    /// Key of the active theme.
    pub theme: String,
}

pub fn run(data: Shared, tx: Sender<Command>, opts: Options) -> Result<()> {
    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableMouseCapture);
    let mut app = App::new(data, tx, opts);
    let res = (|| -> Result<()> {
        while !app.quit {
            app.refresh();
            terminal.draw(|f| draw::draw(f, &mut app))?;
            if event::poll(Duration::from_millis(500))? {
                // Handle everything queued before the next frame.
                loop {
                    match event::read()? {
                        TEvent::Key(k) => app.on_key(k),
                        TEvent::Mouse(m) => app.on_mouse(m),
                        _ => {}
                    }
                    if app.quit || !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }
        Ok(())
    })();
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    res
}
