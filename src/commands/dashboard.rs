//! `aitop`: the interactive dashboard.

use std::time::Duration;

use anyhow::{bail, Result};

use super::Context;
use crate::collector::{self, Collector};
use crate::ui;

pub fn run(ctx: &Context, refresh: Option<f64>) -> Result<()> {
    let secs = refresh.unwrap_or(ctx.cfg.refresh_seconds);
    if !(0.2..=3600.0).contains(&secs) {
        bail!("refresh interval must be between 0.2 and 3600 seconds");
    }
    let collector = Collector::new(&ctx.cfg, ctx.history.clone());
    let (data, tx) = collector::spawn(collector, Duration::from_secs_f64(secs));
    let opts = ui::Options {
        range: ctx.range,
        active_window_ms: ctx.active_window_ms,
        hide_missing: ctx.cfg.hide_missing,
        show_feed: ctx.cfg.feed,
        config_path: ctx.config_path.clone(),
        themes: ctx.themes.clone(),
        budget: ctx.cfg.budget.clone(),
        theme: ctx.theme.clone(),
    };
    ui::run(data, tx, opts)
}
