//! Owns the log sources and the merged, de-duplicated, priced event list.
//!
//! * this module: reading the sources, and merging what any of them (or the
//!   history file) reports into one list;
//! * [`machines`]: usage from other machines and from T3 Code;
//! * [`dataset`]: what is published for the dashboard and commands.

mod dataset;
mod machines;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicI64;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::event::{Event, Interner, Tool};
use crate::host::normalize;
use crate::limits::RateLimits;
use crate::pricing::{PriceTable, Pricer};
use crate::project::ProjectResolver;
use crate::remote::{self, Cursor, Fetched, Remote};
use crate::sources::{self, Ctx, Source, SourceStatus};
use crate::store::Store;
use crate::t3;

pub use dataset::{Dataset, RemoteStatus, Shared, T3Status, VIA_T3};

/// A remote and what the collector tracks for it.
struct Link {
    remote: Remote,
    name: Arc<str>,
    /// Newest record held, shared with the remote's sync thread.
    cursor: Cursor,
    /// Asks the sync thread to run now.
    wake: Option<Sender<()>>,
}

/// Records and newest timestamp per machine.
#[derive(Clone, Copy, Default)]
struct Held {
    records: usize,
    newest: i64,
}

pub struct Collector {
    sources: Vec<Box<dyn Source>>,
    interner: Interner,
    projects: ProjectResolver,
    pricer: Pricer,
    /// id -> [`version`] of the copy we kept.
    seen: HashMap<Arc<str>, u64>,
    store: Option<Store>,
    /// Load the history but never write to it.
    read_only: bool,
    /// Only read log files modified since this time (unix ms).
    since: Option<i64>,
    /// This machine's name.
    host: Arc<str>,
    links: Vec<Link>,
    held: HashMap<Arc<str>, Held>,
    /// Results from the remotes' sync threads.
    fetched: (Sender<Fetched>, Receiver<Fetched>),
    /// The T3 Code client, until its thread takes it.
    t3: Option<t3::Client>,
    t3_every: Duration,
    t3_wake: Option<Sender<()>>,
    t3_read: (Sender<t3::Outcome>, Receiver<t3::Outcome>),
    /// Tools whose folders the config leaves open, so T3's may be added.
    t3_roots_for: Vec<Tool>,
    /// Keep the cost T3 reports for a linked machine rather than pricing it here.
    t3_costs: bool,
    /// Machines (by normalised name) whose usage is held response by
    /// response. A daily total for one of them would count it twice.
    covered: HashSet<String>,
    /// Where `covered` is remembered between runs.
    covered_path: Option<PathBuf>,
    /// Codex sessions read request by request (ids starting `x2:`), and how
    /// many records of the older kind are held.
    recounted: HashSet<Arc<str>>,
    legacy_codex: usize,
    pub data: Shared,
}

impl Collector {
    pub fn new(cfg: &Config, history: Option<PathBuf>) -> Collector {
        let sources = sources::all(cfg);
        let pricer = Pricer::new(PriceTable::load(&cfg.pricing, cfg.t3.use_prices()));
        let mut interner = Interner::default();
        let host = interner.get(&cfg.host_name());
        // The config was checked when it was loaded; one that was not gets no remotes.
        let links: Vec<Link> = remote::resolve(cfg, &host)
            .unwrap_or_default()
            .into_iter()
            .map(|r| Link {
                name: interner.get(&r.name),
                remote: r,
                cursor: Arc::new(AtomicI64::new(remote::NONE)),
                wake: None,
            })
            .collect();
        // This machine and the ssh remotes are read in full detail.
        let covered_path = history.as_ref().and_then(|h| Some(h.parent()?.join("machines.json")));
        let remembered: Vec<String> = covered_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let covered: HashSet<String> = [host.to_string(), crate::host::local(None)]
            .into_iter()
            .chain(links.iter().map(|l| l.remote.name.clone()))
            .chain(remembered)
            .map(|n| normalize(&n))
            .collect();
        let data = Dataset {
            host: host.to_string(),
            remotes: links
                .iter()
                .map(|l| RemoteStatus {
                    name: l.remote.name.clone(),
                    target: l.remote.target.clone(),
                    ..Default::default()
                })
                .collect(),
            pricing_sources: pricer.table().sources.clone(),
            history_path: history.clone(),
            status: all_tools(sources.iter().map(|s| SourceStatus::new(s.tool(), s.roots())).collect()),
            limits: vec![None; Tool::ALL.len()],
            ..Default::default()
        };
        Collector {
            sources,
            interner,
            projects: ProjectResolver::new(),
            pricer,
            seen: HashMap::new(),
            store: history.map(|path| Store::new(path, host.clone())),
            read_only: false,
            since: None,
            host,
            links,
            held: HashMap::new(),
            fetched: mpsc::channel(),
            t3: None,
            t3_every: Duration::from_secs(300),
            t3_wake: None,
            t3_read: mpsc::channel(),
            t3_roots_for: Vec::new(),
            t3_costs: cfg.t3.use_prices(),
            covered,
            covered_path,
            recounted: HashSet::new(),
            legacy_codex: 0,
            data: Arc::new(RwLock::new(data)),
        }
    }

    /// A Codex record made by an older aitop, for a session that has since
    /// been read again request by request (which counts it more exactly).
    fn superseded(&self, e: &Event) -> bool {
        is_legacy_codex(e) && self.recounted.contains(&e.session)
    }

    /// Drop the held records that are now hidden or superseded.
    fn prune(&mut self) {
        let mut d = self.data.write().unwrap();
        let before = d.events.len();
        d.events.retain(|e| !(self.hidden(e) || self.superseded(e)));
        if d.events.len() == before {
            return;
        }
        d.version += 1;
        let mut held: HashMap<Arc<str>, Held> = HashMap::new();
        for e in &d.events {
            let h = held.entry(e.host.clone()).or_insert(Held { records: 0, newest: e.ts });
            h.records += 1;
            h.newest = h.newest.max(e.ts);
        }
        drop(d);
        self.legacy_codex = 0;
        self.held = held;
        self.publish_held();
    }

    /// Only read log files modified at or after `ms`: much faster for
    /// commands that need recent usage only.
    pub fn read_since(&mut self, ms: i64) {
        self.since = Some(ms);
    }

    /// Read the history without adding to it, for commands that may run
    /// while another aitop is writing it.
    pub fn read_only(&mut self) {
        self.read_only = true;
    }

    pub fn host(&self) -> &Arc<str> {
        &self.host
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
        self.drain_remotes();
        let mut events = Vec::new();
        let mut ctx =
            Ctx { interner: &mut self.interner, projects: &mut self.projects, host: &self.host, since: self.since };
        let status = all_tools(self.sources.iter_mut().map(|s| s.scan(&mut ctx, &mut events)).collect());
        let mut limits: Vec<Option<RateLimits>> = self.sources.iter().map(|s| s.rate_limits().cloned()).collect();
        limits.resize(Tool::ALL.len(), None);
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
    fn merge(&mut self, mut incoming: Vec<Event>, persist: bool) -> bool {
        let mut recounted = false;
        for e in incoming.iter().filter(|e| e.id.starts_with("x2:")) {
            recounted |= self.recounted.insert(e.session.clone());
        }
        incoming.retain(|e| !self.hidden(e) && !self.superseded(e));
        let (added, replaced) = self.dedupe(incoming);
        if added.is_empty() && replaced.is_empty() {
            return false;
        }
        if persist {
            self.persist(added.iter().chain(&replaced));
        }
        for e in &added {
            let h = self.held.entry(e.host.clone()).or_insert(Held { records: 0, newest: e.ts });
            h.records += 1;
            h.newest = h.newest.max(e.ts);
        }
        self.legacy_codex += added.iter().filter(|e| is_legacy_codex(e)).count();
        self.data.write().unwrap().insert(added, replaced);
        self.publish_held();
        if recounted && self.legacy_codex > 0 {
            self.prune();
        }
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
            let version = version(&e);
            let known = match self.seen.get(&e.id) {
                // A response is only replaced by a fuller copy of it; a total
                // by any different reading (the latest is what holds).
                Some(&held) if held == version || (e.rollup.is_none() && version <= held) => continue,
                Some(_) => true,
                None => false,
            };
            self.pricer.apply(&mut e);
            self.seen.insert(e.id.clone(), version);
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
        if self.read_only {
            return;
        }
        let Some(store) = &mut self.store else { return };
        let refs: Vec<&Event> = events.collect();
        if let Err(err) = store.append(&refs) {
            self.data.write().unwrap().history_error = Some(err.to_string());
        }
    }
}

/// `status` (one entry per log reader) with an entry, naming no folders, for
/// each tool that has none, so it can be indexed by `Tool::index()`.
fn all_tools(mut status: Vec<SourceStatus>) -> Vec<SourceStatus> {
    status.extend(Tool::ALL[status.len()..].iter().map(|&t| SourceStatus::new(t, &[])));
    status
}

/// A Codex record as aitop made them before 2.3: one per change in a
/// session's running total, which billed a fork's copied history again.
fn is_legacy_codex(e: &Event) -> bool {
    e.tool == Tool::Codex && e.id.starts_with("x:")
}

/// What tells one copy of a record from another with the same id: for a
/// response, how much output it has so far; for a total, everything in it.
fn version(e: &Event) -> u64 {
    use std::hash::{Hash, Hasher};
    let Some(r) = e.rollup else { return e.tokens.output };
    let t = e.tokens;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (t.input, t.output, t.cache_read, t.cache_write, t.cache_write_1h, r.responses, r.sessions).hash(&mut h);
    (r.cost.map(f64::to_bits), r.unpriced).hash(&mut h);
    h.finish()
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
            // This machine's usage first: a response a remote also reports
            // (a synced transcript folder) then stays filed under this one.
            c.scan();
            c.start_remotes();
            loop {
                match rx.recv_timeout(interval) {
                    Ok(Command::Rescan) => c.wake_remotes(),
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(Command::SetInterval(d)) => interval = d,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                c.scan();
            }
        })
        .expect("spawn collector thread");
    (data, tx)
}
