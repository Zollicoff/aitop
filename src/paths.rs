//! Well-known locations.
//!
//! aitop uses XDG-style locations on every Unix (including macOS) so the
//! files are easy to find:
//!
//! * config:  `$XDG_CONFIG_HOME/aitop/config.toml` (default `~/.config/aitop`)
//! * history: `$XDG_DATA_HOME/aitop/history.jsonl` (default `~/.local/share/aitop`)
//! * cache:   `$XDG_CACHE_HOME/aitop/pricing.json` (default `~/.cache/aitop`)

use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Expand a leading `~/`.
pub fn expand(p: &Path) -> PathBuf {
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

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
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
