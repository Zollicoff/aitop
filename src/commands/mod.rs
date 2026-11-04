//! Subcommands. Each module exposes a `run` function.

pub mod dashboard;
pub mod doctor;
pub mod pricing;
pub mod report;
pub mod themes;

use std::path::PathBuf;

use crate::aggregate::Range;
use crate::collector::Collector;
use crate::config::Config;
use crate::theme::ThemeSet;

/// Settings shared by all commands, resolved from the CLI and config file.
pub struct Context {
    pub cfg: Config,
    pub config_path: PathBuf,
    /// History file, or `None` when disabled.
    pub history: Option<PathBuf>,
    pub range: Range,
    pub active_window_ms: i64,
    pub themes: ThemeSet,
    /// Key of the selected theme (already applied).
    pub theme: String,
}

impl Context {
    /// Load history and scan the logs once (for one-shot commands).
    pub fn snapshot(&self) -> Collector {
        let mut c = Collector::new(&self.cfg, self.history.clone());
        c.load_history();
        c.scan();
        c
    }
}
