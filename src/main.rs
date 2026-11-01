//! aitop — a btop-style terminal dashboard for AI coding tool usage.

mod aggregate;
mod collector;
mod config;
mod event;
mod format;
mod pricing;
mod project;
mod report;
mod sources;
mod store;
mod theme;
mod ui;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Result};
use chrono::Local;
use clap::{Parser, Subcommand};

use aggregate::{Range, Sort};
use collector::Collector;
use config::{tilde, Config};
use event::Tool;

#[derive(Parser)]
#[command(name = "aitop", version, about = "Usage and cost dashboard for Claude Code and Codex CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Range to show at startup.
    #[arg(long, value_enum, global = true)]
    range: Option<Range>,
    /// Seconds between rescans.
    #[arg(long, value_name = "SECS")]
    refresh: Option<f64>,
    /// Don't read or write the local history file.
    #[arg(long, global = true)]
    no_history: bool,
    /// Use this config file instead of ~/.config/aitop/config.toml.
    #[arg(long, value_name = "PATH", global = true)]
    config: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print a usage report and exit.
    Report {
        /// Group rows by project, model, session or day.
        #[arg(long, value_enum, default_value = "project")]
        by: report::By,
        /// Only one tool.
        #[arg(long, value_enum)]
        tool: Option<ToolArg>,
        #[arg(long, value_enum, default_value = "cost")]
        sort: Sort,
        /// Maximum rows per tool.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Show or update model prices.
    Pricing {
        #[command(subcommand)]
        action: Option<PricingCmd>,
    },
    /// Show where aitop looks for logs and what it found.
    Doctor,
    /// Print an example config file.
    Config,
}

#[derive(Subcommand)]
enum PricingCmd {
    /// Download current prices (from LiteLLM's public price list).
    Update {
        /// Write to this file instead of the cache.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Look up the price used for a model id.
    Show { model: String },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ToolArg {
    Claude,
    Codex,
}

impl From<ToolArg> for Tool {
    fn from(t: ToolArg) -> Tool {
        match t {
            ToolArg::Claude => Tool::Claude,
            ToolArg::Codex => Tool::Codex,
        }
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("aitop: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref())?;
    let history = (cfg.history && !cli.no_history).then(store::Store::default_path);
    let range = match cli.range {
        Some(r) => r,
        None => Range::parse(&cfg.default_range).unwrap_or(Range::Today),
    };
    let active_ms = (cfg.active_minutes.max(1) * 60_000) as i64;

    match cli.cmd {
        None => {
            let secs = cli.refresh.unwrap_or(cfg.refresh_seconds);
            if !(0.2..=3600.0).contains(&secs) {
                bail!("refresh interval must be between 0.2 and 3600 seconds");
            }
            let collector = Collector::new(&cfg, history);
            let (data, tx) = collector::spawn(collector, Duration::from_secs_f64(secs));
            let opts = ui::Options {
                range,
                active_window_ms: active_ms,
                hide_missing: cfg.hide_missing,
                config_path: cli.config.clone().unwrap_or_else(|| config::config_dir().join("config.toml")),
            };
            ui::run(data, tx, opts)
        }
        Some(Cmd::Report { by, tool, sort, limit, json }) => {
            let mut c = Collector::new(&cfg, history);
            c.load_history();
            c.scan();
            let d = c.data.read().unwrap();
            let summary = aggregate::summarize(&d.events, range, sort, Local::now(), active_ms);
            print!("{}", report::render(&summary, &d.events, by, tool.map(Tool::from), json, limit.max(1)));
            Ok(())
        }
        Some(Cmd::Pricing { action: Some(PricingCmd::Update { output }) }) => {
            let (path, n) = pricing::update(output.as_deref())?;
            println!("saved {n} model prices to {}", tilde(&path));
            Ok(())
        }
        Some(Cmd::Pricing { action: Some(PricingCmd::Show { model }) }) => {
            let t = pricing::PriceTable::load(&cfg.pricing);
            match t.lookup(&model) {
                Some((key, p)) => {
                    println!("{model} → {key}");
                    println!("{}", serde_json::to_string_pretty(p)?);
                }
                None => bail!("no price known for {model:?}; add one under [pricing] in the config"),
            }
            Ok(())
        }
        Some(Cmd::Pricing { action: None }) => pricing_overview(&cfg, history),
        Some(Cmd::Doctor) => doctor(&cfg, history, cli.config),
        Some(Cmd::Config) => {
            print!("{}", config::EXAMPLE);
            Ok(())
        }
    }
}

/// Every model seen in the logs with the price applied to it.
fn pricing_overview(cfg: &Config, history: Option<PathBuf>) -> Result<()> {
    let mut c = Collector::new(cfg, history);
    c.load_history();
    c.scan();
    let d = c.data.read().unwrap();
    let mut seen: BTreeMap<(Tool, String), (u64, f64)> = BTreeMap::new();
    for e in &d.events {
        let s = seen.entry((e.tool, e.model.to_string())).or_default();
        s.0 += e.tokens.total();
        s.1 += e.cost;
    }
    println!("prices: {}  (USD per million tokens)\n", c.pricing().sources.join(" + "));
    println!(
        "{:<8} {:<34} {:<26} {:>7} {:>7} {:>7} {:>7} {:>9}",
        "TOOL", "MODEL", "PRICED AS", "INPUT", "OUTPUT", "C.READ", "C.WRITE", "COST"
    );
    for ((tool, model), (_tok, cost)) in &seen {
        match c.pricing().lookup(model) {
            Some((key, p)) => println!(
                "{:<8} {:<34} {:<26} {:>7} {:>7} {:>7} {:>7} {:>9}",
                tool.key(),
                format::truncate(model, 34),
                format::truncate(key, 26),
                p.input,
                p.output,
                p.cache_read.map(|v| v.to_string()).unwrap_or("-".into()),
                p.cache_write.map(|v| v.to_string()).unwrap_or("-".into()),
                format::cost(*cost)
            ),
            None => println!("{:<8} {:<34} {:<26}", tool.key(), format::truncate(model, 34), "(no price)"),
        }
    }
    if seen.is_empty() {
        println!("(no usage found yet)");
    }
    println!("\n{} models in the price table. Update with `aitop pricing update`.", c.pricing().len());
    Ok(())
}

fn doctor(cfg: &Config, history: Option<PathBuf>, config_override: Option<PathBuf>) -> Result<()> {
    let cfg_path = config_override.unwrap_or_else(|| config::config_dir().join("config.toml"));
    println!("aitop {}", env!("CARGO_PKG_VERSION"));
    println!(
        "config   {} ({})",
        tilde(&cfg_path),
        if cfg_path.exists() { "found" } else { "not present, using defaults" }
    );
    println!("history  {}", history.as_deref().map(tilde).unwrap_or_else(|| "disabled".into()));
    let mut c = Collector::new(cfg, history);
    c.load_history();
    let started = std::time::Instant::now();
    c.scan();
    let elapsed = started.elapsed();
    let d = c.data.read().unwrap();
    println!("pricing  {}", d.pricing_sources.join(" + "));
    println!();
    for st in &d.status {
        let n = d.events.iter().filter(|e| e.tool == st.tool).count();
        println!("{}  ({} files, {} records)", st.tool.name(), st.files, n);
        for (p, exists) in &st.roots {
            println!("  {} {}", if *exists { "✓" } else { "✗" }, tilde(p));
        }
        for e in st.errors.iter().take(5) {
            println!("  ! {e}");
        }
    }
    if let Some(l) = &d.codex_limits {
        println!("\nCodex rate limits (as of {}):", format::duration(Local::now().timestamp_millis() - l.ts) + " ago");
        for w in [&l.primary, &l.secondary].into_iter().flatten() {
            println!("  {:>5.1}% of {} min window", w.used_percent, w.window_minutes.unwrap_or(0));
        }
    }
    if let Some(e) = &d.history_error {
        println!("\nhistory error: {e}");
    }
    println!("\nscan took {} ms", elapsed.as_millis());
    Ok(())
}
