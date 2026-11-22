//! Usage from beyond this machine's own logs: the `[[remote]]`s read over
//! ssh, and what T3 Code reports (its folders, linked machines and other
//! agents). Also where it is decided which of two views of the same usage
//! is kept, so that nothing is counted twice.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use super::{Collector, RemoteStatus, T3Status, VIA_T3};
use crate::config::Config;
use crate::event::{Event, Tool};
use crate::host::normalize;
use crate::remote::{self, Fetched};
use crate::sources::SourceStatus;
use crate::t3;

impl Collector {
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
    pub(super) fn hidden(&self, e: &Event) -> bool {
        // (An agent aitop has no reader for is only ever known by its totals.)
        e.rollup.is_some()
            && e.tool.has_reader()
            && (t3::read_on(e).is_some_and(|h| self.covered.contains(h)) || self.covered.contains(&normalize(&e.host)))
    }

    /// Note that the machines `names` are read in detail, and drop any daily
    /// totals held for them.
    pub(super) fn cover(&mut self, names: impl IntoIterator<Item = String>) {
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

    /// The newest record held per machine name.
    fn newest_held(&self) -> Vec<(String, i64)> {
        self.held.iter().map(|(name, h)| (name.to_string(), h.newest)).collect()
    }

    /// Take in what one read of T3 Code found.
    pub(super) fn merge_t3(&mut self, out: t3::Outcome) {
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
    pub(super) fn drain_remotes(&mut self) {
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
        let (answered, error) = match parsed {
            Ok(batch) => {
                // Known by either name, it is not also taken from T3 Code.
                self.cover([batch.header.host.clone(), batch.header.hostname.clone()]);
                self.merge(batch.events, true);
                (true, None)
            }
            Err(e) => (false, Some(e)),
        };
        let mut d = self.data.write().unwrap();
        let st = &mut d.remotes[f.index];
        if answered {
            st.synced = Some(chrono::Local::now().timestamp_millis());
        }
        st.error = error;
    }

    /// Publish how many records each machine has, and move the remotes'
    /// cursors up to their newest.
    pub(super) fn publish_held(&self) {
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
}
