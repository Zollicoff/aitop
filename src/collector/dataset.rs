//! What the collector publishes for the dashboard and commands to read.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::event::Event;
use crate::limits::RateLimits;
use crate::sources::SourceStatus;

/// How a machine read through T3 Code is described where an ssh remote
/// would show its command.
pub const VIA_T3: &str = "T3 Code";

#[derive(Default)]
pub struct Dataset {
    /// Sorted by timestamp.
    pub events: Vec<Event>,
    /// Indexed by `Tool::index()`.
    pub status: Vec<SourceStatus>,
    /// Latest rate-limit snapshot per tool, indexed by `Tool::index()`.
    pub limits: Vec<Option<RateLimits>>,
    /// Bumped whenever `events` or `limits` change.
    pub version: u64,
    /// True once the first full scan has finished.
    pub loaded: bool,
    pub pricing_sources: Vec<String>,
    pub history_path: Option<PathBuf>,
    pub history_error: Option<String>,
    pub last_scan: Option<Duration>,
    /// This machine's name.
    pub host: String,
    /// The configured remotes, in config order.
    pub remotes: Vec<RemoteStatus>,
    /// How many machines there are records from.
    pub machines: usize,
    /// The T3 Code server on this machine, when aitop reads from one.
    pub t3: Option<T3Status>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct T3Status {
    pub version: String,
    /// True once it has been read at least once this run.
    pub read: bool,
    /// Why the last read failed, if it did.
    pub error: Option<String>,
}

/// How syncing one remote is going.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemoteStatus {
    pub name: String,
    /// How it is reached: `ssh macbook`, or [`VIA_T3`].
    pub target: String,
    /// Records held for it.
    pub records: usize,
    /// Time of its newest record (unix ms).
    pub newest: Option<i64>,
    /// When it last answered (unix ms).
    pub synced: Option<i64>,
    /// Why the last sync failed, if it did.
    pub error: Option<String>,
}

pub type Shared = Arc<RwLock<Dataset>>;

impl Dataset {
    pub(super) fn insert(&mut self, added: Vec<Event>, replaced: Vec<Event>) {
        for r in replaced {
            if let Some(slot) = self.events.iter_mut().rev().find(|x| x.id == r.id) {
                *slot = r;
            }
        }
        let out_of_order = added.first().is_some_and(|a| self.events.last().is_some_and(|l| a.ts < l.ts))
            || !added.is_sorted_by_key(|e| e.ts);
        self.events.extend(added);
        if out_of_order {
            self.events.sort_by_key(|e| e.ts);
        }
        self.version += 1;
    }
}
