//! Core data model: one `Event` per billed model response.

use std::collections::HashSet;
use std::ops::AddAssign;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Claude,
    Codex,
    // The agents below have no log reader in aitop: their usage comes from
    // T3 Code, as daily totals (see [`crate::t3`]).
    Grok,
    Cursor,
    OpenCode,
    Antigravity,
    Pi,
    Muse,
    /// An agent T3 Code tracks that this aitop does not know by name.
    Other,
}

impl Tool {
    pub const ALL: [Tool; 9] = [
        Tool::Claude,
        Tool::Codex,
        Tool::Grok,
        Tool::Cursor,
        Tool::OpenCode,
        Tool::Antigravity,
        Tool::Pi,
        Tool::Muse,
        Tool::Other,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Tool::Claude => "Claude Code",
            Tool::Codex => "Codex",
            Tool::Grok => "Grok Build",
            Tool::Cursor => "Cursor",
            Tool::OpenCode => "OpenCode",
            Tool::Antigravity => "Antigravity",
            Tool::Pi => "Pi",
            Tool::Muse => "Muse Code",
            Tool::Other => "Other agents",
        }
    }

    /// The name used in the config, on the command line and in history
    /// records. For all but `Other` it is also what T3 Code calls the agent.
    pub const fn key(self) -> &'static str {
        match self {
            Tool::Claude => "claude",
            Tool::Codex => "codex",
            Tool::Grok => "grok",
            Tool::Cursor => "cursor",
            Tool::OpenCode => "opencode",
            Tool::Antigravity => "antigravity",
            Tool::Pi => "pi",
            Tool::Muse => "muse",
            Tool::Other => "other",
        }
    }

    /// The tool T3 Code calls `agent`; one it knows and aitop does not is `Other`.
    pub fn from_agent(agent: &str) -> Tool {
        Tool::ALL.into_iter().find(|t| t.key() == agent).unwrap_or(Tool::Other)
    }

    /// Whether aitop reads this tool's own logs (and so knows its projects,
    /// sessions and single responses).
    pub fn has_reader(self) -> bool {
        matches!(self, Tool::Claude | Tool::Codex)
    }

    /// Whether the tool's subscription is metered in 5-hour windows.
    pub fn has_usage_windows(self) -> bool {
        self == Tool::Claude
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
    /// The machine the response was made on (see [`crate::host`]).
    pub host: Arc<str>,
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
    /// The speed tier the response ran at, which changes its price.
    pub speed: Speed,
    /// Estimated cost in USD (filled in by the collector from the price table).
    pub cost: f64,
    /// False when no price was found for `model`.
    pub priced: bool,
    /// Set when this is not one response but a total another program
    /// reported (see [`crate::t3`]): a machine's usage of one model on one day.
    pub rollup: Option<Rollup>,
}

/// The speed tier of a response: Claude's fast mode, Codex's service tiers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Speed {
    #[default]
    Standard,
    /// Claude "fast mode"; Codex "priority" (also called fast).
    Fast,
    Ultrafast,
}

/// What a summarised [`Event`] stands for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rollup {
    pub responses: u32,
    /// Sessions that took part; their ids are not known.
    pub sessions: u32,
    /// The cost as the reporting program worked it out, response by
    /// response. `None`: price the totals here instead.
    pub cost: Option<f64>,
    /// Some of the responses had no price there, so `cost` leaves them out.
    pub unpriced: bool,
}

impl Event {
    /// How many responses this event counts for.
    pub fn responses(&self) -> u64 {
        self.rollup.map_or(1, |r| u64::from(r.responses))
    }
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
    fn tools() {
        assert_eq!(Tool::ALL.map(Tool::index), [0, 1, 2, 3, 4, 5, 6, 7, 8], "ALL is in index order");
        assert_eq!((Tool::from_agent("grok"), Tool::from_agent("pi")), (Tool::Grok, Tool::Pi));
        assert_eq!(Tool::from_agent("someday"), Tool::Other);
        assert!(Tool::Codex.has_reader() && !Tool::Cursor.has_reader());
        assert_eq!(serde_json::to_string(&Tool::OpenCode).unwrap(), "\"opencode\"");
    }

    #[test]
    fn interner_dedupes() {
        let mut i = Interner::default();
        let a = i.get("x");
        let b = i.get("x");
        assert!(Arc::ptr_eq(&a, &b));
    }
}
