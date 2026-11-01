//! Table rows and the accumulator that builds them.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;

use super::range::Sort;
use crate::event::{Event, Tokens};

#[derive(Clone, Debug, Default, Serialize)]
pub struct Row {
    pub key: String,
    pub name: String,
    /// Secondary text: project path, session id or top model.
    pub detail: String,
    pub sessions: usize,
    pub messages: u64,
    pub tokens: Tokens,
    pub cost: f64,
    /// Some usage in this row has no known price.
    pub unpriced: bool,
    pub first_ts: i64,
    pub last_ts: i64,
}

/// Accumulates events into one [`Row`].
#[derive(Default)]
pub(super) struct Acc {
    name: Arc<str>,
    detail: Arc<str>,
    tokens: Tokens,
    cost: f64,
    messages: u64,
    sessions: HashSet<Arc<str>>,
    /// Cost per model (plus a tiny token-based tiebreak for unpriced models).
    models: HashMap<Arc<str>, f64>,
    unpriced: bool,
    first_ts: i64,
    last_ts: i64,
}

impl Acc {
    pub(super) fn named(name: &Arc<str>, detail: &Arc<str>) -> Acc {
        Acc { name: name.clone(), detail: detail.clone(), ..Default::default() }
    }

    pub(super) fn add(&mut self, e: &Event) {
        if self.messages == 0 {
            self.first_ts = e.ts;
        }
        self.tokens += e.tokens;
        self.cost += e.cost;
        self.messages += 1;
        self.sessions.insert(e.session.clone());
        *self.models.entry(e.model.clone()).or_default() += e.cost + 1e-12 * e.tokens.total() as f64;
        self.unpriced |= !e.priced;
        self.first_ts = self.first_ts.min(e.ts);
        self.last_ts = self.last_ts.max(e.ts);
    }

    pub(super) fn tokens(&self) -> Tokens {
        self.tokens
    }

    pub(super) fn cost(&self) -> f64 {
        self.cost
    }

    pub(super) fn messages(&self) -> u64 {
        self.messages
    }

    pub(super) fn session_count(&self) -> usize {
        self.sessions.len()
    }

    pub(super) fn detail(&self) -> String {
        self.detail.to_string()
    }

    pub(super) fn top_model(&self) -> String {
        self.models.iter().max_by(|a, b| a.1.total_cmp(b.1)).map(|(m, _)| m.to_string()).unwrap_or_default()
    }

    pub(super) fn into_row(self, key: &str, detail: String) -> Row {
        Row {
            key: key.to_string(),
            name: self.name.to_string(),
            detail,
            sessions: self.sessions.len(),
            messages: self.messages,
            tokens: self.tokens,
            cost: self.cost,
            unpriced: self.unpriced,
            first_ts: self.first_ts,
            last_ts: self.last_ts,
        }
    }
}

/// Events grouped by a key (project path, model, session id…).
#[derive(Default)]
pub(super) struct Groups(HashMap<Arc<str>, Acc>);

impl Groups {
    pub(super) fn add(&mut self, key: &Arc<str>, name: &Arc<str>, detail: &Arc<str>, e: &Event) {
        self.0.entry(key.clone()).or_insert_with(|| Acc::named(name, detail)).add(e);
    }

    /// One row per group; `detail` picks each row's secondary text.
    pub(super) fn into_rows(self, sort: Sort, detail: impl Fn(&Acc) -> String) -> Vec<Row> {
        let mut rows: Vec<Row> = self
            .0
            .into_iter()
            .map(|(k, a)| {
                let d = detail(&a);
                a.into_row(&k, d)
            })
            .collect();
        sort_rows(&mut rows, sort);
        rows
    }
}

pub fn sort_rows(rows: &mut [Row], sort: Sort) {
    match sort {
        Sort::Cost => rows.sort_by(|a, b| b.cost.total_cmp(&a.cost).then(b.tokens.total().cmp(&a.tokens.total()))),
        Sort::Tokens => rows.sort_by_key(|r| std::cmp::Reverse(r.tokens.total())),
        Sort::Recent => rows.sort_by_key(|r| std::cmp::Reverse(r.last_ts)),
        Sort::Name => rows.sort_by_key(|r| r.name.to_lowercase()),
    }
}

/// Two different repositories with the same folder name get their parent
/// directory prepended so they can be told apart.
pub(super) fn disambiguate(rows: &mut [Row]) {
    let mut count: HashMap<String, usize> = HashMap::new();
    for r in rows.iter() {
        *count.entry(r.name.clone()).or_default() += 1;
    }
    for r in rows.iter_mut() {
        if count[&r.name] > 1 {
            let parent =
                Path::new(&r.key).parent().and_then(|p| p.file_name()).map(|p| p.to_string_lossy().to_string());
            if let Some(p) = parent {
                r.name = format!("{p}/{}", r.name);
            }
        }
    }
}
