//! aitop — a btop-style terminal dashboard for AI coding tool usage.

mod aggregate;
mod budget;
mod cli;
mod collector;
mod commands;
mod config;
mod event;
mod format;
mod limits;
mod paths;
mod pricing;
mod project;
mod sources;
mod store;
mod theme;
mod ui;

use anyhow::Result;
use clap::Parser;

use aggregate::Range;
use cli::{Cli, Cmd, PricingCmd, ThemesCmd};
use commands::Context;
use config::Config;

fn main() {
    if let Err(e) = run() {
        eprintln!("aitop: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref())?;
    let themes = theme::ThemeSet::load(&paths::config_dir().join("themes"));
    let selected = themes.get(cli.theme.as_deref().or(cfg.theme.as_deref()).unwrap_or("aitop"))?.clone();
    theme::set(selected.palette);
    let ctx = Context {
        history: (cfg.history && !cli.no_history).then(store::Store::default_path),
        range: cli.range.or_else(|| Range::parse(&cfg.default_range)).unwrap_or(Range::Today),
        active_window_ms: (cfg.active_minutes.max(1) * 60_000) as i64,
        config_path: cli.config.clone().unwrap_or_else(paths::config_file),
        cfg,
        themes,
        theme: selected.key,
    };
    match cli.cmd {
        None => commands::dashboard::run(&ctx, cli.refresh),
        Some(Cmd::Report(args)) => commands::report::run(&ctx, &args),
        Some(Cmd::Pricing { action: None }) => commands::pricing::overview(&ctx),
        Some(Cmd::Pricing { action: Some(PricingCmd::Update { output }) }) => {
            commands::pricing::update(output.as_deref())
        }
        Some(Cmd::Pricing { action: Some(PricingCmd::Show { model }) }) => commands::pricing::show(&ctx, &model),
        Some(Cmd::Statusline(args)) => commands::statusline::run(&ctx, &args),
        Some(Cmd::Themes { action: None }) => commands::themes::list(&ctx),
        Some(Cmd::Themes { action: Some(ThemesCmd::Show { name }) }) => commands::themes::show(&ctx, &name),
        Some(Cmd::Doctor) => commands::doctor::run(&ctx),
        Some(Cmd::Config) => {
            print!("{}", config::EXAMPLE);
            Ok(())
        }
    }
}
