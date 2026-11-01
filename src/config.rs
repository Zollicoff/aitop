//! Configuration file and well-known locations.
//!
//! aitop uses XDG-style locations on every Unix (including macOS) so the
//! files are easy to find:
//!
//! * config:  `$XDG_CONFIG_HOME/aitop/config.toml` (default `~/.config/aitop`)
//! * history: `$XDG_DATA_HOME/aitop/history.jsonl` (default `~/.local/share/aitop`)
//! * cache:   `$XDG_CACHE_HOME/aitop/pricing.json` (default `~/.cache/aitop`)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::pricing::Price;

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
    pub paths: PathsConfig,
    /// Per-model price overrides in USD per million tokens.
    pub pricing: BTreeMap<String, Price>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PathsConfig {
    pub claude: Vec<PathBuf>,
    pub codex: Vec<PathBuf>,
    pub gemini: Vec<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            refresh_seconds: 2.0,
            default_range: "today".into(),
            history: true,
            active_minutes: 5,
            hide_missing: false,
            paths: PathsConfig::default(),
            pricing: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Config> {
        let (path, explicit) = match path {
            Some(p) => (p.to_path_buf(), true),
            None => (config_dir().join("config.toml"), false),
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("invalid config file {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    pub fn claude_roots(&self) -> Vec<PathBuf> {
        if !self.paths.claude.is_empty() {
            return self.paths.claude.iter().map(|p| expand(p)).collect();
        }
        if let Ok(dirs) = std::env::var("CLAUDE_CONFIG_DIR") {
            let v: Vec<PathBuf> = dirs
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| expand(Path::new(s)).join("projects"))
                .collect();
            if !v.is_empty() {
                return v;
            }
        }
        vec![home().join(".config/claude/projects"), home().join(".claude/projects")]
    }

    pub fn codex_roots(&self) -> Vec<PathBuf> {
        if !self.paths.codex.is_empty() {
            return self.paths.codex.iter().map(|p| expand(p)).collect();
        }
        let base = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"));
        vec![base.join("sessions"), base.join("archived_sessions")]
    }

    pub fn gemini_roots(&self) -> Vec<PathBuf> {
        if !self.paths.gemini.is_empty() {
            return self.paths.gemini.iter().map(|p| expand(p)).collect();
        }
        let base = std::env::var_os("GEMINI_CLI_HOME")
            .map(|p| PathBuf::from(p).join(".gemini"))
            .unwrap_or_else(|| home().join(".gemini"));
        vec![base.join("tmp")]
    }
}

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn expand(p: &Path) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_prefix("~/")) {
        Some(rest) => home().join(rest),
        None => p.to_path_buf(),
    }
}

fn xdg(var: &str, fallback: &str, win: Option<PathBuf>) -> PathBuf {
    if let Some(v) = std::env::var_os(var).filter(|v| !v.is_empty()) {
        return PathBuf::from(v).join("aitop");
    }
    if cfg!(windows) {
        if let Some(w) = win {
            return w.join("aitop");
        }
    }
    home().join(fallback).join("aitop")
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config", dirs::config_dir())
}

pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share", dirs::data_dir())
}

pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache", dirs::cache_dir())
}

/// Replace the home directory prefix with `~` for display.
pub fn tilde(p: &Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
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

# Override where logs are read from (defaults shown).
# [paths]
# claude = ["~/.claude/projects", "~/.config/claude/projects"]
# codex  = ["~/.codex/sessions", "~/.codex/archived_sessions"]
# gemini = ["~/.gemini/tmp"]

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
}
