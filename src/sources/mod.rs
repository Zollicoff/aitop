//! Log readers, one module per tool.
//!
//! Every reader implements [`Source`]. Readers are incremental: a file is only
//! read from where the previous scan stopped, so polling every couple of
//! seconds stays cheap even with gigabytes of history. Adding a tool means
//! adding a module here, a variant to [`Tool`], and an entry in [`all`].

pub mod claude;
pub mod codex;
mod files;
mod tail;

use std::path::PathBuf;

use crate::config::Config;
use crate::event::{Event, Interner, Tool};
use crate::limits::RateLimits;
use crate::project::ProjectResolver;

pub use files::{discover, parse_ts, prune};
pub use tail::{Change, Tail};

/// Shared state handed to readers while scanning.
pub struct Ctx<'a> {
    pub interner: &'a mut Interner,
    pub projects: &'a mut ProjectResolver,
    /// Skip files not modified since this time (unix ms). Logs are append
    /// only, so such files hold nothing newer. Used by one-shot commands
    /// that only need recent usage.
    pub since: Option<i64>,
}

impl Ctx<'_> {
    /// Drop files that can't contain events at or after `since`.
    pub fn recent(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        match self.since {
            Some(s) => paths.into_iter().filter(|p| files::modified(p).is_none_or(|m| m >= s)).collect(),
            None => paths,
        }
    }
}

pub trait Source: Send {
    fn tool(&self) -> Tool;
    /// Directories this source reads from.
    fn roots(&self) -> &[PathBuf];
    /// Read whatever is new since the last call, pushing events into `out`.
    fn scan(&mut self, ctx: &mut Ctx, out: &mut Vec<Event>) -> SourceStatus;
    /// Latest account rate-limit snapshot, for tools that log one.
    fn rate_limits(&self) -> Option<&RateLimits> {
        None
    }
}

/// Every supported tool, in display order (matching [`Tool::ALL`]).
pub fn all(cfg: &Config) -> Vec<Box<dyn Source>> {
    let roots = |t: Tool, default: fn() -> Vec<PathBuf>| cfg.roots_override(t).unwrap_or_else(default);
    vec![
        Box::new(claude::ClaudeSource::new(roots(Tool::Claude, claude::default_roots))),
        Box::new(codex::CodexSource::new(roots(Tool::Codex, codex::default_roots))),
    ]
}

#[derive(Clone, Debug)]
pub struct SourceStatus {
    pub tool: Tool,
    /// Root directories and whether they exist.
    pub roots: Vec<(PathBuf, bool)>,
    pub files: usize,
    pub errors: Vec<String>,
}

impl SourceStatus {
    pub fn new(tool: Tool, roots: &[PathBuf]) -> Self {
        SourceStatus {
            tool,
            roots: roots.iter().map(|r| (r.clone(), r.is_dir())).collect(),
            files: 0,
            errors: Vec::new(),
        }
    }

    pub fn detected(&self) -> bool {
        self.roots.iter().any(|(_, e)| *e)
    }

    pub fn error(&mut self, path: &std::path::Path, e: impl std::fmt::Display) {
        self.errors.push(format!("{}: {e}", path.display()));
    }
}
