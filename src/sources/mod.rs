//! Log readers, one module per tool.
//!
//! Every reader implements [`Source`]. Readers are incremental: a file is only
//! read from where the previous scan stopped, so polling every couple of
//! seconds stays cheap even with gigabytes of history. Adding a tool means
//! adding a module here, a variant to [`Tool`], and an entry in [`all`].
//!
//! Not every [`Tool`] has a reader: the agents aitop only knows through
//! T3 Code have none, and their usage arrives as totals (see [`crate::t3`]).

pub mod claude;
pub mod codex;
mod files;
mod tail;

use std::path::PathBuf;
use std::sync::Arc;

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
    /// Name of this machine, stamped on every event read here.
    pub host: &'a Arc<str>,
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
    /// Also read from `roots` (ones already read are ignored).
    fn add_roots(&mut self, roots: &[PathBuf]);
    /// Read whatever is new since the last call, pushing events into `out`.
    fn scan(&mut self, ctx: &mut Ctx, out: &mut Vec<Event>) -> SourceStatus;
    /// Latest account rate-limit snapshot, for tools that log one.
    fn rate_limits(&self) -> Option<&RateLimits> {
        None
    }
}

/// `roots` with those of `more` it does not already have, comparing the
/// folders they resolve to.
pub fn merge_roots(roots: &mut Vec<PathBuf>, more: &[PathBuf]) {
    let real = |p: &PathBuf| p.canonicalize().unwrap_or_else(|_| p.clone());
    for m in more {
        if !roots.iter().any(|r| real(r) == real(m)) {
            roots.push(m.clone());
        }
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
