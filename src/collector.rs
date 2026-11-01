//! Owns the log sources and the merged, de-duplicated, priced event list.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::event::{Event, Interner, Tool};
use crate::pricing::{Price, PriceTable};
use crate::project::ProjectResolver;
use crate::sources::claude::ClaudeSource;
use crate::sources::codex::{CodexSource, RateLimits};
use crate::sources::gemini::GeminiSource;
use crate::sources::{Ctx, Source, SourceStatus};
use crate::store::Store;

#[derive(Default)]
pub struct Dataset {
    /// Sorted by timestamp.
    pub events: Vec<Event>,
    /// Indexed by `Tool::index()`.
    pub status: Vec<SourceStatus>,
    pub codex_limits: Option<RateLimits>,
    /// Bumped whenever `events` changes.
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
    claude: ClaudeSource,
    codex: CodexSource,
    gemini: GeminiSource,
    interner: Interner,
    projects: ProjectResolver,
    pricing: PriceTable,
    price_memo: HashMap<Arc<str>, Option<Price>>,
    /// id -> output tokens of the copy we kept.
    seen: HashMap<Arc<str>, u64>,
    store: Option<Store>,
    pub data: Shared,
}

impl Collector {
    pub fn new(cfg: &Config, history: Option<PathBuf>) -> Collector {
        let pricing = PriceTable::load(&cfg.pricing);
        let data = Dataset {
            pricing_sources: pricing.sources.clone(),
            history_path: history.clone(),
            status: vec![
                SourceStatus::new(Tool::Claude, &cfg.claude_roots()),
                SourceStatus::new(Tool::Codex, &cfg.codex_roots()),
                SourceStatus::new(Tool::Gemini, &cfg.gemini_roots()),
            ],
            ..Default::default()
        };
        Collector {
            claude: ClaudeSource::new(cfg.claude_roots()),
            codex: CodexSource::new(cfg.codex_roots()),
            gemini: GeminiSource::new(cfg.gemini_roots()),
            interner: Interner::default(),
            projects: ProjectResolver::new(),
            pricing,
            price_memo: HashMap::new(),
            seen: HashMap::new(),
            store: history.map(Store::new),
            data: Arc::new(RwLock::new(data)),
        }
    }

    pub fn pricing(&self) -> &PriceTable {
        &self.pricing
    }

    /// Load the local history file (if enabled).
    pub fn load_history(&mut self) {
        let Some(store) = &self.store else { return };
        match store.load(&mut self.interner) {
            Ok(events) => {
                // Make sure history projects help resolve Gemini hashes too.
                for e in &events {
                    if e.project_path.starts_with('/') {
                        self.projects.resolve(&e.project_path, &mut self.interner);
                    }
                }
                self.merge(events, false);
            }
            Err(e) => self.data.write().unwrap().history_error = Some(e.to_string()),
        }
    }

    /// Scan all sources once. Returns true if anything changed.
    pub fn scan(&mut self) -> bool {
        let started = Instant::now();
        let mut events = Vec::new();
        let mut ctx = Ctx { interner: &mut self.interner, projects: &mut self.projects };
        let st_claude = self.claude.scan(&mut ctx, &mut events);
        let st_codex = self.codex.scan(&mut ctx, &mut events);
        let st_gemini = self.gemini.scan(&mut ctx, &mut events);
        let changed = self.merge(events, true);
        let mut d = self.data.write().unwrap();
        d.status = vec![st_claude, st_codex, st_gemini];
        if d.codex_limits != self.codex.limits {
            d.codex_limits = self.codex.limits.clone();
            d.version += 1;
        }
        if !d.loaded {
            d.loaded = true;
            d.version += 1;
        }
        d.last_scan = Some(started.elapsed());
        changed
    }

    fn price(&mut self, e: &mut Event) {
        let p = match self.price_memo.get(&e.model) {
            Some(p) => *p,
            None => {
                let p = self.pricing.lookup(&e.model).map(|(_, p)| *p);
                self.price_memo.insert(e.model.clone(), p);
                p
            }
        };
        match p {
            Some(p) => {
                e.cost = p.cost(&e.tokens, e.fast);
                e.priced = true;
            }
            None => {
                e.cost = 0.0;
                e.priced = false;
            }
        }
    }

    fn merge(&mut self, incoming: Vec<Event>, persist: bool) -> bool {
        if incoming.is_empty() {
            return false;
        }
        let mut added: Vec<Event> = Vec::new();
        let mut replaced: Vec<Event> = Vec::new();
        for mut e in incoming {
            match self.seen.get(&e.id) {
                Some(&out) if e.tokens.output <= out => continue,
                Some(_) => {
                    self.price(&mut e);
                    self.seen.insert(e.id.clone(), e.tokens.output);
                    // A later duplicate in the same batch replaces the earlier one.
                    if let Some(a) = added.iter_mut().rev().find(|a| a.id == e.id) {
                        *a = e;
                    } else {
                        replaced.push(e);
                    }
                }
                None => {
                    self.price(&mut e);
                    self.seen.insert(e.id.clone(), e.tokens.output);
                    added.push(e);
                }
            }
        }
        if added.is_empty() && replaced.is_empty() {
            return false;
        }
        if persist {
            if let Some(store) = &mut self.store {
                let refs: Vec<&Event> = added.iter().chain(replaced.iter()).collect();
                if let Err(err) = store.append(&refs) {
                    self.data.write().unwrap().history_error = Some(err.to_string());
                }
            }
        }
        let mut d = self.data.write().unwrap();
        for r in replaced {
            if let Some(slot) = d.events.iter_mut().rev().find(|x| x.id == r.id) {
                *slot = r;
            }
        }
        let needs_sort = added.first().is_some_and(|a| d.events.last().is_some_and(|l| a.ts < l.ts))
            || !added.is_sorted_by_key(|e| e.ts);
        d.events.extend(added);
        if needs_sort {
            d.events.sort_by_key(|e| e.ts);
        }
        d.version += 1;
        true
    }
}

pub enum Command {
    Rescan,
}

/// Run the collector on a background thread, rescanning every `interval`.
pub fn spawn(mut c: Collector, interval: Duration) -> (Shared, Sender<Command>) {
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
                gemini: vec![dir.path().join("none")],
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
