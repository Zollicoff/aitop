//! Core data model: one `Event` per billed model response.

use std::collections::HashSet;
use std::ops::AddAssign;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Claude,
    Codex,
}

impl Tool {
    pub const ALL: [Tool; 2] = [Tool::Claude, Tool::Codex];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Claude => "Claude Code",
            Tool::Codex => "Codex",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Tool::Claude => "claude",
            Tool::Codex => "codex",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Token counts for one response, normalised across tools so that the
/// fields are disjoint and each one maps to exactly one price:
///
/// * `input`          – uncached prompt tokens
/// * `output`         – generated tokens (including reasoning/thinking)
/// * `cache_read`     – prompt tokens served from cache
/// * `cache_write`    – prompt tokens written to a 5-minute cache
/// * `cache_write_1h` – prompt tokens written to a 1-hour cache
/// * `reasoning`      – informational subset of `output`
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cache_write_1h: u64,
    pub reasoning: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write + self.cache_write_1h
    }

    pub fn cache_writes(&self) -> u64 {
        self.cache_write + self.cache_write_1h
    }

    /// Tokens that made up the prompt for this request.
    pub fn prompt(&self) -> u64 {
        self.input + self.cache_read + self.cache_write + self.cache_write_1h
    }

    pub fn is_zero(&self) -> bool {
        self.total() == 0
    }
}

impl AddAssign for Tokens {
    fn add_assign(&mut self, o: Tokens) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
        self.cache_write_1h += o.cache_write_1h;
        self.reasoning += o.reasoning;
    }
}

#[derive(Clone, Debug)]
pub struct Event {
    pub tool: Tool,
    /// Unique, stable id (prefixed with the tool key) used for de-duplication.
    pub id: Arc<str>,
    /// Unix time in milliseconds.
    pub ts: i64,
    pub session: Arc<str>,
    /// Display name of the project (usually the git repository name).
    pub project: Arc<str>,
    /// Absolute path of the project root.
    pub project_path: Arc<str>,
    pub model: Arc<str>,
    pub tokens: Tokens,
    /// Claude "fast mode" responses are billed at a premium.
    pub fast: bool,
    /// Estimated cost in USD (filled in by the collector from the price table).
    pub cost: f64,
    /// False when no price was found for `model`.
    pub priced: bool,
}

/// String interner so that the (very repetitive) session, project and model
/// strings are only stored once.
#[derive(Default)]
pub struct Interner(HashSet<Arc<str>>);

impl Interner {
    pub fn get(&mut self, s: &str) -> Arc<str> {
        if let Some(a) = self.0.get(s) {
            return a.clone();
        }
        let a: Arc<str> = Arc::from(s);
        self.0.insert(a.clone());
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_total_and_add() {
        let mut a = Tokens { input: 1, output: 2, cache_read: 3, cache_write: 4, cache_write_1h: 5, reasoning: 1 };
        assert_eq!(a.total(), 15);
        a += a;
        assert_eq!(a.total(), 30);
        assert_eq!(a.reasoning, 2);
    }

    #[test]
    fn interner_dedupes() {
        let mut i = Interner::default();
        let a = i.get("x");
        let b = i.get("x");
        assert!(Arc::ptr_eq(&a, &b));
    }
}
