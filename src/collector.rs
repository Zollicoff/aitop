//! Owns the log sources and the merged, de-duplicated, priced event list.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::event::{Event, Interner};
use crate::limits::RateLimits;
use crate::pricing::{PriceTable, Pricer};
use crate::project::ProjectResolver;
use crate::sources::{self, Ctx, Source, SourceStatus};
use crate::store::Store;

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
}

pub type Shared = Arc<RwLock<Dataset>>;

pub struct Collector {
    sources: Vec<Box<dyn Source>>,
    interner: Interner,
    projects: ProjectResolver,
    pricer: Pricer,
    /// id -> output tokens of the copy we kept.
    seen: HashMap<Arc<str>, u64>,
    store: Option<Store>,
    /// Only read log files modified since this time (unix ms).
    since: Option<i64>,
    pub data: Shared,
}

impl Collector {
    pub fn new(cfg: &Config, history: Option<PathBuf>) -> Collector {
        let sources = sources::all(cfg);
        let pricer = Pricer::new(PriceTable::load(&cfg.pricing));
        let data = Dataset {
            pricing_sources: pricer.table().sources.clone(),
            history_path: history.clone(),
            status: sources.iter().map(|s| SourceStatus::new(s.tool(), s.roots())).collect(),
            limits: vec![None; sources.len()],
            ..Default::default()
        };
        Collector {
            sources,
            interner: Interner::default(),
            projects: ProjectResolver::new(),
            pricer,
            seen: HashMap::new(),
            store: history.map(Store::new),
            since: None,
            data: Arc::new(RwLock::new(data)),
        }
    }

    /// Only read log files modified at or after `ms`: much faster for
    /// commands that need recent usage only.
    pub fn read_since(&mut self, ms: i64) {
        self.since = Some(ms);
    }

    pub fn pricing(&self) -> &PriceTable {
        self.pricer.table()
    }

    /// Load the local history file (if enabled).
    pub fn load_history(&mut self) {
        let Some(store) = &self.store else { return };
        match store.load(&mut self.interner) {
            Ok(events) => {
                self.merge(events, false);
            }
            Err(e) => self.data.write().unwrap().history_error = Some(e.to_string()),
        }
    }

    /// Scan all sources once. Returns true if anything changed.
    pub fn scan(&mut self) -> bool {
        let started = Instant::now();
        let mut events = Vec::new();
        let mut ctx = Ctx { interner: &mut self.interner, projects: &mut self.projects, since: self.since };
        let status: Vec<SourceStatus> = self.sources.iter_mut().map(|s| s.scan(&mut ctx, &mut events)).collect();
        let limits: Vec<Option<RateLimits>> = self.sources.iter().map(|s| s.rate_limits().cloned()).collect();
        let changed = self.merge(events, true);
        let mut d = self.data.write().unwrap();
        d.status = status;
        if d.limits != limits {
            d.limits = limits;
            d.version += 1;
        }
        if !d.loaded {
            d.loaded = true;
            d.version += 1;
        }
        d.last_scan = Some(started.elapsed());
        changed
    }

    /// Add newly read events: de-duplicate, price, persist, insert.
    fn merge(&mut self, incoming: Vec<Event>, persist: bool) -> bool {
        let (added, replaced) = self.dedupe(incoming);
        if added.is_empty() && replaced.is_empty() {
            return false;
        }
        if persist {
            self.persist(added.iter().chain(&replaced));
        }
        self.data.write().unwrap().insert(added, replaced);
        true
    }

    /// Split `incoming` into new events and better copies of known ones (a
    /// streamed Claude response is logged several times with growing output),
    /// dropping everything already seen.
    fn dedupe(&mut self, incoming: Vec<Event>) -> (Vec<Event>, Vec<Event>) {
        let mut added: Vec<Event> = Vec::new();
        let mut replaced: Vec<Event> = Vec::new();
        // Where each id sits in `added`, so a later copy in the same batch
        // replaces the earlier one without a scan.
        let mut in_batch: HashMap<Arc<str>, usize> = HashMap::new();
        for mut e in incoming {
            let known = match self.seen.get(&e.id) {
                Some(&out) if e.tokens.output <= out => continue,
                Some(_) => true,
                None => false,
            };
            self.pricer.apply(&mut e);
            self.seen.insert(e.id.clone(), e.tokens.output);
            if let Some(&i) = in_batch.get(&e.id) {
                added[i] = e;
            } else if known {
                replaced.push(e);
            } else {
                in_batch.insert(e.id.clone(), added.len());
                added.push(e);
            }
        }
        (added, replaced)
    }

    fn persist<'a>(&mut self, events: impl Iterator<Item = &'a Event>) {
        let Some(store) = &mut self.store else { return };
        let refs: Vec<&Event> = events.collect();
        if let Err(err) = store.append(&refs) {
            self.data.write().unwrap().history_error = Some(err.to_string());
        }
    }
}

impl Dataset {
    fn insert(&mut self, added: Vec<Event>, replaced: Vec<Event>) {
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

pub enum Command {
    Rescan,
    /// Change how often the logs are rescanned.
    SetInterval(Duration),
}

/// Run the collector on a background thread, rescanning every `interval`.
pub fn spawn(mut c: Collector, mut interval: Duration) -> (Shared, Sender<Command>) {
    let data = c.data.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("aitop-collector".into())
        .spawn(move || {
            c.load_history();
            loop {
                c.scan();
                match rx.recv_timeout(interval) {
                    Ok(Command::Rescan) | Err(RecvTimeoutError::Timeout) => {}
                    Ok(Command::SetInterval(d)) => interval = d,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .expect("spawn collector thread");
    (data, tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupe_keeps_largest_output_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("claude/-x-proj");
        std::fs::create_dir_all(&logs).unwrap();
        let mk = |out: u64| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-11-01T10:00:00Z","sessionId":"s","requestId":"r","message":{{"id":"m","model":"claude-sonnet-4-5","usage":{{"input_tokens":100000,"output_tokens":{out}}}}}}}"#
            )
        };
        std::fs::write(logs.join("s.jsonl"), format!("{}\n{}\n", mk(10), mk(1_000_000))).unwrap();
        let cfg = Config {
            paths: crate::config::PathsConfig {
                claude: vec![dir.path().join("claude")],
                codex: vec![dir.path().join("none")],
            },
            ..Default::default()
        };
        let hist = dir.path().join("history.jsonl");
        let mut c = Collector::new(&cfg, Some(hist.clone()));
        c.load_history();
        c.scan();
        {
            let d = c.data.read().unwrap();
            assert_eq!(d.events.len(), 1);
            assert_eq!(d.events[0].tokens.output, 1_000_000);
            assert!((d.events[0].cost - 15.3).abs() < 1e-9, "sonnet 4.5: $0.30 in + $15 out");
        }
        // Logs deleted: history still has the record.
        std::fs::remove_dir_all(dir.path().join("claude")).unwrap();
        let mut c2 = Collector::new(&cfg, Some(hist));
        c2.load_history();
        c2.scan();
        let d = c2.data.read().unwrap();
        assert_eq!(d.events.len(), 1);
        assert_eq!(d.events[0].tokens.output, 1_000_000);
    }
}
