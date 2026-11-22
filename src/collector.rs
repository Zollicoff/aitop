//! Owns the log sources and the merged, de-duplicated, priced event list.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
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
    /// What the machine calls itself.
    pub reported_host: Option<String>,
}

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

pub type Shared = Arc<RwLock<Dataset>>;

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

    /// Read from T3 Code as the config allows: the transcript folders it
    /// reported before are added now, and [`Collector::start_remotes`] or
    /// [`Collector::sync_remotes`] ask it again and read its linked machines.
    /// `state` is where aitop keeps its T3 sessions.
    pub fn use_t3(&mut self, cfg: &Config, state: PathBuf) {
        if !cfg.t3.enabled {
            return;
        }
        self.t3_roots_for = Tool::ALL.into_iter().filter(|t| cfg.roots_override(*t).is_none()).collect();
        self.add_roots(&t3::cached_roots(&state));
        self.t3_every = cfg.t3.interval();
        self.t3 = t3::Client::detect(state);
        if let Some(client) = &self.t3 {
            let status = T3Status { version: client.version().to_string(), ..Default::default() };
            self.data.write().unwrap().t3 = Some(status);
        }
    }

    /// Have the sources read `roots` too, where the config leaves that open.
    fn add_roots(&mut self, roots: &[(Tool, PathBuf)]) {
        for source in &mut self.sources {
            let tool = source.tool();
            if self.t3_roots_for.contains(&tool) {
                let more: Vec<PathBuf> = roots.iter().filter(|(t, _)| *t == tool).map(|(_, p)| p.clone()).collect();
                source.add_roots(&more);
            }
        }
        let mut d = self.data.write().unwrap();
        for (st, source) in d.status.iter_mut().zip(&self.sources) {
            st.roots = SourceStatus::new(source.tool(), source.roots()).roots;
        }
    }

    /// A daily total for a machine whose usage is already held in detail.
    fn hidden(&self, e: &Event) -> bool {
        // (An agent aitop has no reader for is only ever known by its totals.)
        e.rollup.is_some()
            && e.tool.has_reader()
            && (t3::read_on(e).is_some_and(|h| self.covered.contains(h)) || self.covered.contains(&normalize(&e.host)))
    }

    /// Note that the machines `names` are read in detail, and drop any daily
    /// totals held for them.
    fn cover(&mut self, names: impl IntoIterator<Item = String>) {
        let mut grew = false;
        for name in names {
            let name = normalize(&name);
            grew |= name != "local" && self.covered.insert(name);
        }
        if !grew {
            return;
        }
        if let (Some(path), false) = (&self.covered_path, self.read_only) {
            let mut names: Vec<&String> = self.covered.iter().collect();
            names.sort();
            let _ = std::fs::write(path, serde_json::to_string(&names).unwrap_or_default());
        }
        self.prune();
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

    /// The newest record held per machine name.
    fn newest_held(&self) -> Vec<(String, i64)> {
        self.held.iter().map(|(name, h)| (name.to_string(), h.newest)).collect()
    }

    /// Take in what one read of T3 Code found.
    fn merge_t3(&mut self, out: t3::Outcome) {
        self.add_roots(&out.roots);
        self.cover(out.local_hosts);
        let mut error = out.error;
        match out.local {
            Some(Ok(totals)) => {
                let events = t3::local_events(&self.host.clone(), &totals, &mut self.interner, self.t3_costs);
                self.merge(events, true);
            }
            Some(Err(e)) => error = error.or(Some(format!("other agents: {e}"))),
            None => {}
        }
        let now = chrono::Local::now().timestamp_millis();
        for machine in out.machines {
            let mut covered = self.covered.contains(&normalize(&machine.name));
            let (error, synced) = match &machine.result {
                Ok(totals) => {
                    // Everything it reported was read on machines held in detail.
                    let mut readable = totals.iter().filter(|t| t.total.tool.has_reader()).peekable();
                    covered |=
                        readable.peek().is_some() && readable.all(|t| self.covered.contains(&normalize(&t.total.host)));
                    let events = t3::events(&machine, totals, &mut self.interner, self.t3_costs);
                    self.merge(events, true);
                    (None, Some(now))
                }
                Err(e) => (Some(e.clone()), None),
            };
            let mut d = self.data.write().unwrap();
            let at = d.remotes.iter().position(|r| r.name == machine.name && r.target == VIA_T3);
            if covered {
                if let Some(at) = at {
                    d.remotes.remove(at);
                }
                continue;
            }
            let at = at.unwrap_or_else(|| {
                d.remotes.push(RemoteStatus {
                    name: machine.name.clone(),
                    target: VIA_T3.into(),
                    ..Default::default()
                });
                d.remotes.len() - 1
            });
            d.remotes[at].error = error;
            d.remotes[at].synced = synced.or(d.remotes[at].synced);
        }
        self.data.write().unwrap().t3 = Some(T3Status { version: out.version, read: true, error });
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

    /// Start syncing the remotes in the background; [`Collector::scan`]
    /// merges what they report. Call after [`Collector::load_history`], so
    /// each sync starts from what is already held.
    pub fn start_remotes(&mut self) {
        for (index, link) in self.links.iter_mut().enumerate() {
            if link.wake.is_none() {
                let tx = self.fetched.0.clone();
                link.wake = Some(remote::spawn(index, link.remote.clone(), link.cursor.clone(), tx));
            }
        }
        if let Some(mut client) = self.t3.take() {
            client.seed(self.newest_held());
            self.t3_wake = Some(t3::spawn(client, self.t3_every, self.t3_read.0.clone()));
        }
    }

    /// Ask every remote to sync now rather than at its next interval.
    pub fn wake_remotes(&self) {
        for wake in self.links.iter().filter_map(|l| l.wake.as_ref()).chain(&self.t3_wake) {
            let _ = wake.send(());
        }
    }

    /// Sync every remote once, waiting for each, and return the outcomes
    /// (also left in `data`).
    pub fn sync_remotes(&mut self) -> Vec<RemoteStatus> {
        for index in 0..self.links.len() {
            let link = &self.links[index];
            let newest = Some(link.cursor.load(Ordering::Relaxed)).filter(|&c| c != remote::NONE);
            let result = link.remote.fetch(remote::since(newest));
            self.merge_fetched(Fetched { index, result });
        }
        if let Some(mut client) = self.t3.take() {
            client.seed(self.newest_held());
            let read = client.sync();
            self.t3 = Some(client);
            self.merge_t3(read);
        }
        self.data.read().unwrap().remotes.clone()
    }

    /// Merge what the sync threads have reported since the last call.
    fn drain_remotes(&mut self) {
        while let Ok(f) = self.fetched.1.try_recv() {
            self.merge_fetched(f);
        }
        while let Ok(read) = self.t3_read.1.try_recv() {
            self.merge_t3(read);
        }
    }

    fn merge_fetched(&mut self, f: Fetched) {
        let name = self.links[f.index].name.clone();
        let parsed = f.result.and_then(|out| remote::parse(&out, &mut self.interner, &name));
        let (reported, error) = match parsed {
            Ok(batch) => {
                // Known by either name, it is not also taken from T3 Code.
                self.cover([batch.header.host.clone(), batch.header.hostname.clone()]);
                self.merge(batch.events, true);
                (Some(batch.header.host), None)
            }
            Err(e) => (None, Some(e)),
        };
        let mut d = self.data.write().unwrap();
        let st = &mut d.remotes[f.index];
        if let Some(host) = reported {
            st.reported_host = Some(host);
            st.synced = Some(chrono::Local::now().timestamp_millis());
        }
        st.error = error;
    }

    /// Publish how many records each machine has, and move the remotes'
    /// cursors up to their newest.
    fn publish_held(&self) {
        let mut d = self.data.write().unwrap();
        d.machines = self.held.len();
        for st in &mut d.remotes {
            let held = self.held.get(st.name.as_str());
            st.records = held.map_or(0, |h| h.records);
            st.newest = held.map(|h| h.newest);
        }
        for link in &self.links {
            if let Some(h) = self.held.get(&link.name) {
                link.cursor.store(h.newest, Ordering::Relaxed);
            }
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
            host: Some("desk".into()),
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
        assert_eq!((&*d.events[0].host, d.machines), ("desk", 1));
    }

    /// A config whose only remote, `laptop`, answers with the file `export`.
    fn with_remote(dir: &std::path::Path, export: &std::path::Path) -> Config {
        Config {
            paths: crate::config::PathsConfig { claude: vec![dir.join("claude")], codex: vec![dir.join("none")] },
            host: Some("desk".into()),
            remote: vec![crate::remote::RemoteConfig {
                name: Some("laptop".into()),
                // `export` and its flags land in $1.. and are ignored.
                command: ["sh", "-c", "cat \"$0\"", &export.to_string_lossy()].map(String::from).to_vec(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn remote_usage_merges_once_under_its_machine() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("claude/-x-proj");
        std::fs::create_dir_all(&logs).unwrap();
        let line = |id: &str, ts: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","sessionId":"s","requestId":"r","message":{{"id":"{id}","model":"claude-sonnet-4-5","usage":{{"input_tokens":1000,"output_tokens":10}}}}}}"#
            )
        };
        std::fs::write(logs.join("s.jsonl"), line("shared", "2026-11-01T10:00:00Z") + "\n").unwrap();

        // What the laptop exports: one response this machine also has (a
        // synced transcript) and one of its own.
        let export = dir.path().join("export.jsonl");
        let mut i = Interner::default();
        let laptop = |i: &mut Interner, id: &str, ts: i64| Event {
            id: i.get(id),
            model: i.get("claude-sonnet-4-5"),
            ..crate::aggregate::testutil::ev(i, crate::event::Tool::Claude, ts, "/home/z/web", "ls", 0.0)
        };
        let (shared, own) = (laptop(&mut i, "c:shared:r", 1), laptop(&mut i, "c:own:r", 1_900_000_000_000));
        let mut out = Vec::new();
        remote::write_export(&mut out, "sams-laptop", &[&shared, &own]).unwrap();
        std::fs::write(&export, out).unwrap();

        let cfg = with_remote(dir.path(), &export);
        let hist = dir.path().join("history.jsonl");
        let mut c = Collector::new(&cfg, Some(hist.clone()));
        c.load_history();
        c.scan();
        let st = c.sync_remotes();
        assert_eq!(st[0].error, None);
        assert_eq!((st[0].records, st[0].newest), (1, Some(1_900_000_000_000)), "the shared response stays local");
        assert_eq!(st[0].reported_host.as_deref(), Some("sams-laptop"));
        {
            let d = c.data.read().unwrap();
            let hosts: Vec<&str> = d.events.iter().map(|e| &*e.host).collect();
            assert_eq!(hosts, ["desk", "laptop"]);
            assert_eq!(d.machines, 2);
            assert!(d.events[1].priced, "remote records are priced here");
        }
        // Syncing again adds nothing, and the next sync starts near the newest record.
        assert_eq!(c.sync_remotes()[0].records, 1);
        assert_eq!(c.links[0].cursor.load(Ordering::Relaxed), 1_900_000_000_000);

        // The laptop's usage is kept: with the remote gone, history still has it.
        std::fs::remove_file(&export).unwrap();
        let mut c2 = Collector::new(&cfg, Some(hist));
        c2.load_history();
        let st = c2.sync_remotes();
        assert!(st[0].error.is_some(), "an unreachable remote reports why");
        assert_eq!(st[0].records, 1);
        let d = c2.data.read().unwrap();
        assert_eq!(d.events.iter().filter(|e| &*e.host == "laptop").count(), 1);
    }

    fn day_total(host: &str, day: &str, ts: i64, output: u64) -> t3::DayTotal {
        let tokens = crate::event::Tokens { input: 100, output, ..Default::default() };
        let total = t3::Total {
            host: host.into(),
            tool: Tool::Claude,
            model: "claude-sonnet-4-5".into(),
            day: day.into(),
            tokens,
            responses: 12,
            sessions: 2,
            cost: 4.5,
            unpriced: 0,
            agent: "claude".into(),
        };
        t3::DayTotal { total, ts }
    }

    #[test]
    fn t3_machines_merge_without_counting_twice() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = with_remote(dir.path(), &dir.path().join("missing"));
        let hist = dir.path().join("history.jsonl");
        let mut c = Collector::new(&cfg, Some(hist.clone()));
        c.t3_roots_for = vec![Tool::Claude];
        let extra = dir.path().join("t3-claude/projects");
        std::fs::create_dir_all(&extra).unwrap();

        let machine = |id: &str, name: &str, result| t3::Machine { id: id.into(), name: name.into(), result };
        let read = |machines| t3::Outcome {
            version: "0.1.12".into(),
            roots: vec![(Tool::Claude, extra.clone()), (Tool::Codex, dir.path().join("t3-codex"))],
            local_hosts: vec!["Desk-Box.local".into()],
            machines,
            ..Default::default()
        };
        c.merge_t3(read(vec![
            machine(
                "e1",
                "Studio",
                Ok(vec![
                    day_total("studio.local", "2026-11-14", 1000, 5),
                    day_total("studio.local", "2026-11-15", 2000, 7),
                ]),
            ),
            // A link back to this very machine, and one to the ssh remote: both already held in detail.
            machine("e2", "Desk again", Ok(vec![day_total("Desk-Box.local", "2026-11-15", 2000, 9)])),
            machine("e3", "Laptop", Ok(vec![day_total("laptop.local", "2026-11-15", 2000, 9)])),
            machine("e4", "Server", Err("not reachable (asleep or offline)".into())),
        ]));
        {
            let d = c.data.read().unwrap();
            let shown: Vec<(&str, bool, usize, bool)> =
                d.remotes.iter().map(|r| (r.name.as_str(), r.target == VIA_T3, r.records, r.error.is_some())).collect();
            assert_eq!(shown, [("laptop", false, 0, false), ("Studio", true, 2, false), ("Server", true, 0, true)]);
            assert_eq!(d.events.len(), 2, "only Studio's totals are kept");
            assert!(d.events.iter().all(|e| &*e.host == "Studio" && e.priced && e.responses() == 12));
            assert!(d.events.iter().all(|e| e.cost == 4.5), "the cost T3 reported is kept");
            assert_eq!(d.t3, Some(T3Status { version: "0.1.12".into(), read: true, error: None }));
            // T3's Claude folder is read from now on; Codex's is fixed by the config.
            assert!(d.status[Tool::Claude.index()].roots.iter().any(|(p, _)| *p == extra));
            assert_eq!(d.status[Tool::Codex.index()].roots.len(), 1);
        }
        // A day that is read again replaces what was held, and history keeps the totals.
        c.merge_t3(read(vec![machine("e1", "Studio", Ok(vec![day_total("studio.local", "2026-11-15", 2000, 70)]))]));
        assert_eq!(c.data.read().unwrap().events.iter().map(|e| e.tokens.output).collect::<Vec<_>>(), [5, 70]);
        // Also when the new reading is lower, or only its cost changed.
        let mut repriced = day_total("studio.local", "2026-11-15", 2000, 60);
        repriced.total.cost = 9.0;
        c.merge_t3(read(vec![machine("e1", "Studio", Ok(vec![repriced.clone()]))]));
        c.merge_t3(read(vec![machine("e1", "Studio", Ok(vec![repriced]))]));
        {
            let d = c.data.read().unwrap();
            assert_eq!(d.events.iter().map(|e| (e.tokens.output, e.cost)).collect::<Vec<_>>(), [(5, 4.5), (60, 9.0)]);
        }
        let mut c2 = Collector::new(&cfg, Some(hist.clone()));
        c2.load_history();
        assert_eq!(
            c2.data.read().unwrap().events.iter().map(|e| (e.tokens.output, e.cost)).collect::<Vec<_>>(),
            [(5, 4.5), (60, 9.0)]
        );

        // Once Studio is also read over ssh, its daily totals step aside, now and on later runs.
        c.cover(["studio".to_string()]);
        let d = c.data.read().unwrap();
        assert!(d.events.is_empty());
        assert_eq!(d.remotes.iter().find(|r| r.name == "Studio").unwrap().records, 0);
        drop(d);
        let mut c3 = Collector::new(&cfg, Some(hist));
        c3.load_history();
        assert!(c3.data.read().unwrap().events.is_empty(), "remembered in machines.json");
    }

    #[test]
    fn other_agents_come_from_t3_also_for_this_machine() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = with_remote(dir.path(), &dir.path().join("missing"));
        let hist = dir.path().join("history.jsonl");
        let mut c = Collector::new(&cfg, Some(hist.clone()));
        let grok = |host: &str, output: u64| {
            let mut t = day_total(host, "2026-11-15", 2000, output);
            t.total.tool = Tool::Grok;
            t.total.agent = "grok".into();
            t.total.model = "grok-build".into();
            t
        };
        let read = t3::Outcome {
            version: "0.1.12".into(),
            local_hosts: vec!["Desk-Box.local".into()],
            // This machine's own Grok usage, and the same reported back over a link to it.
            local: Some(Ok(vec![grok("Desk-Box.local", 5)])),
            machines: vec![
                t3::Machine { id: "e2".into(), name: "Desk again".into(), result: Ok(vec![grok("Desk-Box.local", 5)]) },
                t3::Machine { id: "e1".into(), name: "Studio".into(), result: Ok(vec![grok("studio.local", 8)]) },
            ],
            ..Default::default()
        };
        c.merge_t3(read);
        let d = c.data.read().unwrap();
        let seen: Vec<(&str, &str, Tool, u64)> =
            d.events.iter().map(|e| (&*e.host, &*e.project, e.tool, e.tokens.output)).collect();
        // Kept although this machine is read in detail (aitop has no Grok reader), and only once.
        assert_eq!(seen, [("desk", "Grok Build", Tool::Grok, 5), ("Studio", "Grok Build", Tool::Grok, 8)]);
        assert!(d.events.iter().all(|e| e.cost == 4.5 && e.priced));
        assert_eq!(d.status.len(), Tool::ALL.len());
        assert!(d.status[Tool::Grok.index()].roots.is_empty());
        drop(d);
        // It survives a restart through the history file.
        let mut c2 = Collector::new(&cfg, Some(hist));
        c2.load_history();
        assert_eq!(c2.data.read().unwrap().events.len(), 2);
    }

    #[test]
    fn recounted_codex_sessions_replace_their_old_records() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = with_remote(dir.path(), &dir.path().join("missing"));
        let mut c = Collector::new(&cfg, None);
        let mut i = Interner::default();
        let codex = |i: &mut Interner, id: &str, session: &str, cost: f64| Event {
            id: i.get(id),
            ..crate::aggregate::testutil::ev(i, Tool::Codex, 1000, "/code/api", session, cost)
        };
        // History from an older aitop: two sessions, one of which no longer has logs.
        let old = vec![codex(&mut i, "x:kept:500", "kept", 1.0), codex(&mut i, "x:gone:900", "gone", 2.0)];
        c.merge(old, false);
        assert_eq!(c.data.read().unwrap().events.len(), 2);
        // The scan reads `kept` again, request by request.
        c.merge(vec![codex(&mut i, "x2:kept:1000:5.0.5:1", "kept", 0.5)], false);
        let ids = |c: &Collector| c.data.read().unwrap().events.iter().map(|e| e.id.to_string()).collect::<Vec<_>>();
        assert_eq!(ids(&c), ["x:gone:900", "x2:kept:1000:5.0.5:1"]);
        // Its old records stay out, also when history is loaded in one go.
        c.merge(vec![codex(&mut i, "x:kept:700", "kept", 1.0)], false);
        assert_eq!(ids(&c).len(), 2);
        let mut c2 = Collector::new(&cfg, None);
        let mixed = vec![codex(&mut i, "x:kept:500", "kept", 1.0), codex(&mut i, "x2:kept:1000:5.0.5:1", "kept", 0.5)];
        c2.merge(mixed, false);
        assert_eq!(ids(&c2), ["x2:kept:1000:5.0.5:1"]);
    }

    #[test]
    fn read_only_leaves_history_alone() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("claude/-x-proj");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(
            logs.join("s.jsonl"),
            r#"{"type":"assistant","timestamp":"2026-11-01T10:00:00Z","sessionId":"s","requestId":"r","message":{"id":"m","model":"claude-sonnet-4-5","usage":{"input_tokens":1,"output_tokens":1}}}"#.to_string() + "\n",
        )
        .unwrap();
        let cfg = with_remote(dir.path(), &dir.path().join("missing"));
        let hist = dir.path().join("history.jsonl");
        let mut c = Collector::new(&cfg, Some(hist.clone()));
        c.read_only();
        c.load_history();
        c.scan();
        assert_eq!(c.data.read().unwrap().events.len(), 1);
        assert!(!hist.exists());
    }
}
