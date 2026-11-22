//! The optional `config.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::appearance::{BoxStyle, ChartMetric, GraphColor, GraphSymbol, TrueColor};
use crate::budget::BudgetConfig;
use crate::event::Tool;
use crate::paths::{config_file, expand};
use crate::pricing::Price;
use crate::remote::RemoteConfig;
use crate::t3::T3Config;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Seconds between log rescans.
    pub refresh_seconds: f64,
    /// Range shown at startup: today, week, month or all.
    pub default_range: String,
    /// Keep a local copy of every usage record so totals survive log cleanup.
    pub history: bool,
    /// A session counts as active if it produced output within this many minutes.
    pub active_minutes: u64,
    /// Hide tools that are not installed instead of showing a placeholder.
    pub hide_missing: bool,
    /// Start with the activity feed open (toggle with `a`).
    pub feed: bool,
    /// Colour theme: a built-in name or a file in `~/.config/aitop/themes`.
    pub theme: Option<String>,
    /// Paint the theme's background. Off keeps the terminal's own, so a
    /// transparent or translucent terminal shows through.
    pub theme_background: bool,
    pub truecolor: TrueColor,
    pub box_style: BoxStyle,
    pub graph_symbol: GraphSymbol,
    pub graph_color: GraphColor,
    /// What the per-tool chart plots.
    pub chart: ChartMetric,
    /// strftime format for the header clock; empty hides it.
    pub clock_format: String,
    /// What to call this machine when several are shown together.
    pub host: Option<String>,
    /// Other machines whose usage is pulled in (`[[remote]]`).
    pub remote: Vec<RemoteConfig>,
    /// Reading from T3 Code, when it is installed.
    pub t3: T3Config,
    pub paths: PathsConfig,
    pub budget: BudgetConfig,
    /// Per-model price overrides in USD per million tokens.
    pub pricing: BTreeMap<String, Price>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PathsConfig {
    pub claude: Vec<PathBuf>,
    pub codex: Vec<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            refresh_seconds: 2.0,
            default_range: "today".into(),
            history: true,
            active_minutes: 5,
            hide_missing: false,
            feed: false,
            theme: None,
            theme_background: true,
            truecolor: TrueColor::Auto,
            box_style: BoxStyle::Rounded,
            graph_symbol: GraphSymbol::Block,
            graph_color: GraphColor::Accent,
            chart: ChartMetric::Cost,
            clock_format: "%H:%M:%S".into(),
            host: None,
            remote: Vec::new(),
            t3: T3Config::default(),
            paths: PathsConfig::default(),
            budget: BudgetConfig::default(),
            pricing: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Config> {
        let (path, explicit) = match path {
            Some(p) => (p.to_path_buf(), true),
            None => (config_file(), false),
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let invalid = || format!("invalid config file {}", path.display());
                let cfg: Config = toml::from_str(&text).with_context(invalid)?;
                crate::remote::resolve(&cfg, &cfg.host_name()).with_context(invalid)?;
                Ok(cfg)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    /// This machine's name: `host`, or else its hostname.
    pub fn host_name(&self) -> String {
        crate::host::local(self.host.as_deref())
    }

    /// Log directories configured for `tool`, if the user overrode the defaults.
    pub fn roots_override(&self, tool: Tool) -> Option<Vec<PathBuf>> {
        let v = match tool {
            Tool::Claude => &self.paths.claude,
            Tool::Codex => &self.paths.codex,
        };
        (!v.is_empty()).then(|| v.iter().map(|p| expand(p)).collect())
    }
}

pub const EXAMPLE: &str = r#"# aitop configuration — ~/.config/aitop/config.toml

# Seconds between log rescans (scans are incremental, so this is cheap).
refresh_seconds = 2.0

# Range shown at startup: "today", "week", "month" or "all".
default_range = "today"

# Keep a local copy of every usage record in ~/.local/share/aitop so totals
# survive the tools cleaning up their own logs (Claude Code deletes
# transcripts after 30 days by default).
history = true

# A session counts as "active" if it produced output in the last N minutes.
active_minutes = 5

# Hide tools that are not installed instead of showing a placeholder.
hide_missing = false

# Start with the live activity feed open (press `a` to toggle it).
feed = false

# Colour theme: aitop (default), catppuccin, dracula, gruvbox, nord,
# solarized-light, terminal (uses your terminal's own 16 colours) or
# tokyo-night. Custom themes go in ~/.config/aitop/themes/NAME.toml; run
# `aitop themes` to list them and `aitop themes show dracula` for a template.
# theme = "aitop"

# Paint the theme's background colour. Set to false to keep the terminal's
# own background, e.g. to let a transparent or translucent terminal show.
theme_background = true

# Colour depth: "auto" (detect), "on" (24-bit), "off" (256 colours).
truecolor = "auto"

# Panel borders: "rounded", "plain", "double" or "thick".
box_style = "rounded"

# Graphs: "braille" (finest), "block" or "tty" (shade characters for a bare
# console); coloured by tool ("accent") or by height ("gradient"); plotting
# "cost" or "tokens".
graph_symbol = "block"
graph_color = "accent"
chart = "cost"

# Header clock (strftime format); "" hides it.
clock_format = "%H:%M:%S"

# What this machine is called when usage from several machines is shown
# together (see [[remote]] below). Default: its hostname.
# host = "desktop"

# Spend budgets in USD, for today / this week / this month. Budgets under
# [budget] cover all tools; [budget.claude] and [budget.codex] cover one.
# A budget turns amber at warn_at percent and red when it is used up.
# [budget]
# daily = 20.0
# monthly = 300.0
# warn_at = 80
# week_starts = "monday"   # or "sunday"
# bell = false             # ring the terminal bell when a budget changes level
#
# [budget.claude]
# weekly = 100.0

# T3 Code (https://github.com/pingdotgg/t3code). When it is installed and
# running, aitop asks it which transcript folders it uses on this machine and
# reads those too, and shows the usage of the machines linked to it (T3's
# Linked environments), a total per model and day. Nothing to set up: aitop
# gives itself read-only access with T3's own `t3 auth session issue`, listed
# in T3 under Sharing & access as "aitop". Set enabled = false to leave T3 alone.
# [t3]
# enabled = true
# every = 300              # seconds between reads

# Usage from your other machines. Each one listed here is asked for its
# usage over ssh (it needs an aitop that has `aitop export`, and ssh access
# without a password prompt) and shown alongside this machine's. What it reports is
# kept in the local history, so it stays visible while that machine is
# asleep. A response seen from two machines is only counted once.
# [[remote]]
# ssh = "macbook"          # a host, ssh alias or user@host
# name = "macbook"         # what to call it (default: the ssh host)
# aitop = "/usr/local/bin/aitop"   # if aitop is not on the PATH there
# every = 60               # seconds between syncs

# Override where logs are read from (defaults shown).
# [paths]
# claude = ["~/.claude/projects", "~/.config/claude/projects"]
# codex  = ["~/.codex/sessions", "~/.codex/archived_sessions"]

# Override or add model prices, in USD per million tokens.
# [pricing."my-custom-model"]
# input = 1.0
# output = 4.0
# cache_read = 0.1
# cache_write = 1.25
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_parses() {
        let c: Config = toml::from_str(EXAMPLE).unwrap();
        assert_eq!(c.refresh_seconds, 2.0);
        assert!(c.history);
    }

    #[test]
    fn pricing_override_parses() {
        let c: Config = toml::from_str("[pricing.\"foo\"]\ninput = 1.0\noutput = 2.0\n").unwrap();
        assert_eq!(c.pricing["foo"].output, 2.0);
    }

    #[test]
    fn remotes_are_checked_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "host = \"desk\"\n\n[[remote]]\nssh = \"laptop\"\n").unwrap();
        let c = Config::load(Some(&p)).unwrap();
        assert_eq!((c.host_name().as_str(), c.remote.len()), ("desk", 1));
        std::fs::write(&p, "[[remote]]\nssh = \"laptop\"\ncommand = [\"x\"]\n").unwrap();
        let err = format!("{:#}", Config::load(Some(&p)).unwrap_err());
        assert!(err.contains("[[remote]] #1") && err.contains("not both"), "{err}");
    }
}
