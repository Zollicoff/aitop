//! Command-line arguments.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::aggregate::Range;
use crate::commands::report;

#[derive(Parser)]
#[command(name = "aitop", version, about = "Usage and cost dashboard for Claude Code and Codex CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
    /// Range to show at startup.
    #[arg(long, value_enum, global = true)]
    pub range: Option<Range>,
    /// Seconds between rescans.
    #[arg(long, value_name = "SECS")]
    pub refresh: Option<f64>,
    /// Don't read or write the local history file.
    #[arg(long, global = true)]
    pub no_history: bool,
    /// Colour theme (see `aitop themes`).
    #[arg(long, value_name = "NAME", global = true)]
    pub theme: Option<String>,
    /// Use this config file instead of ~/.config/aitop/config.toml.
    #[arg(long, value_name = "PATH", global = true)]
    pub config: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Print a usage report and exit.
    Report(report::Args),
    /// Show or update model prices.
    Pricing {
        #[command(subcommand)]
        action: Option<PricingCmd>,
    },
    /// List colour themes, or print one as a starting point for your own.
    Themes {
        #[command(subcommand)]
        action: Option<ThemesCmd>,
    },
    /// Show where aitop looks for logs and what it found.
    Doctor,
    /// Print an example config file.
    Config,
}

#[derive(Subcommand)]
pub enum PricingCmd {
    /// Download current prices (from LiteLLM's public price list).
    Update {
        /// Write to this file instead of the cache.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Look up the price used for a model id.
    Show { model: String },
}

#[derive(Subcommand)]
pub enum ThemesCmd {
    /// Print a theme as TOML, e.g. `aitop themes show dracula > ~/.config/aitop/themes/mine.toml`.
    Show { name: String },
}
