//! Model price table and cost calculation.
//!
//! Prices are USD per million tokens. Three layers are merged, later ones
//! winning: the table compiled into the binary (`prices.json`), a table
//! downloaded with `aitop pricing update`, and `[pricing]` overrides from the
//! config file.

mod litellm;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::event::{Event, Tokens};

pub use litellm::update;

pub(super) const BUILTIN: &str = include_str!("prices.json");

/// Prompts above this many tokens use the `*_above_200k` tier when a model has one.
const LONG_CONTEXT: u64 = 200_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    /// 5-minute cache write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_above_200k: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_above_200k: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_above_200k: Option<f64>,
    /// Multiplier applied to fast-mode responses (Claude).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_multiplier: Option<f64>,
}

impl Price {
    pub fn cost(&self, t: &Tokens, fast: bool) -> f64 {
        let long = t.prompt() > LONG_CONTEXT;
        let input = if long { self.input_above_200k.unwrap_or(self.input) } else { self.input };
        let output = if long { self.output_above_200k.unwrap_or(self.output) } else { self.output };
        let base_cr = self.cache_read.unwrap_or(self.input * 0.1);
        let cache_read = if long { self.cache_read_above_200k.unwrap_or(base_cr) } else { base_cr };
        let cw = self.cache_write.unwrap_or(input * 1.25);
        let cw1h = self.cache_write_1h.unwrap_or(input * 2.0);
        let usd = (t.input as f64 * input
            + t.output as f64 * output
            + t.cache_read as f64 * cache_read
            + t.cache_write as f64 * cw
            + t.cache_write_1h as f64 * cw1h)
            / 1e6;
        if fast {
            usd * self.fast_multiplier.unwrap_or(1.0)
        } else {
            usd
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PriceFile {
    pub as_of: String,
    pub source: String,
    pub models: BTreeMap<String, Price>,
}

pub struct PriceTable {
    models: HashMap<String, Price>,
    /// Human-readable description of where the prices came from.
    pub sources: Vec<String>,
}

impl PriceTable {
    pub fn builtin() -> PriceTable {
        let f: PriceFile = serde_json::from_str(BUILTIN).expect("built-in pricing.json is valid");
        PriceTable { sources: vec![format!("built-in ({})", f.as_of)], models: f.models.into_iter().collect() }
    }

    /// Built-in table, plus the downloaded table if present, plus overrides.
    pub fn load(overrides: &BTreeMap<String, Price>) -> PriceTable {
        let mut t = PriceTable::builtin();
        if let Ok(text) = std::fs::read_to_string(cache_path()) {
            if let Ok(f) = serde_json::from_str::<PriceFile>(&text) {
                t.sources.push(format!("downloaded {} ({} models)", f.as_of, f.models.len()));
                t.models.extend(f.models);
            }
        }
        if !overrides.is_empty() {
            t.sources.push(format!("{} config override(s)", overrides.len()));
            t.models.extend(overrides.iter().map(|(k, v)| (k.to_lowercase(), *v)));
        }
        t
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Find the price for a model id as it appears in logs. Returns the
    /// matched table key as well, so callers can show what was used.
    pub fn lookup(&self, model: &str) -> Option<(&str, &Price)> {
        let n = normalize(model);
        if n.is_empty() {
            return None;
        }
        for cand in [n.clone(), strip_date(&n)] {
            if let Some((k, p)) = self.models.get_key_value(cand.as_str()) {
                return Some((k.as_str(), p));
            }
        }
        // Fall back to the longest table key that is a dash-separated
        // prefix, e.g. `gpt-5-codex-2025-09-15-preview` -> `gpt-5-codex`.
        let base = strip_date(&n);
        self.models
            .iter()
            .filter(|(k, _)| base.len() > k.len() && base.starts_with(k.as_str()) && base.as_bytes()[k.len()] == b'-')
            .max_by_key(|(k, _)| k.len())
            .map(|(k, p)| (k.as_str(), p))
    }
}

/// Applies a [`PriceTable`] to events, remembering the lookup per model.
pub struct Pricer {
    table: PriceTable,
    memo: HashMap<Arc<str>, Option<Price>>,
}

impl Pricer {
    pub fn new(table: PriceTable) -> Pricer {
        Pricer { table, memo: HashMap::new() }
    }

    pub fn table(&self) -> &PriceTable {
        &self.table
    }

    /// Fill in `cost` and `priced`.
    pub fn apply(&mut self, e: &mut Event) {
        let table = &self.table;
        let price = *self.memo.entry(e.model.clone()).or_insert_with(|| table.lookup(&e.model).map(|(_, p)| *p));
        e.priced = price.is_some();
        e.cost = price.map(|p| p.cost(&e.tokens, e.fast)).unwrap_or(0.0);
    }
}

/// Lower-case and strip provider prefixes and decorations seen in the wild
/// (`anthropic/…`, `models/…`, `…@20250101`, `…[1m]`, Bedrock ids).
fn normalize(model: &str) -> String {
    let mut s = model.trim().to_lowercase();
    if let Some(i) = s.rfind('/') {
        s = s[i + 1..].to_string();
    }
    if let Some(i) = s.find('[') {
        s.truncate(i);
    }
    if let Some(i) = s.find('@') {
        s.truncate(i);
    }
    if let Some(i) = s.find("claude-") {
        s = s[i..].to_string();
    }
    if let Some(stripped) = s.strip_suffix(":0") {
        s = stripped.trim_end_matches("-v1").trim_end_matches("-v2").to_string();
    }
    s
}

fn strip_date(s: &str) -> String {
    // -20251101
    if let Some(i) = s.rfind('-') {
        let tail = &s[i + 1..];
        if tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()) {
            return s[..i].to_string();
        }
    }
    // -2025-11-01
    if s.len() > 11 {
        let (head, tail) = s.split_at(s.len() - 11);
        let b = tail.as_bytes();
        if b[0] == b'-' && b[5] == b'-' && b[8] == b'-' && tail[1..].bytes().filter(|c| c.is_ascii_digit()).count() == 8
        {
            return head.to_string();
        }
    }
    s.to_string()
}

pub fn cache_path() -> PathBuf {
    crate::paths::cache_dir().join("pricing.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> PriceTable {
        PriceTable::builtin()
    }

    #[test]
    fn builtin_has_current_models() {
        let t = table();
        for m in ["claude-opus-5-5", "claude-sonnet-4-5", "claude-haiku-4-5", "gpt-5-codex", "gpt-5.1-codex"] {
            assert!(t.lookup(m).is_some(), "missing {m}");
        }
    }

    #[test]
    fn lookup_normalizes() {
        let t = table();
        assert_eq!(t.lookup("claude-opus-4-5-20251101").unwrap().1, t.lookup("claude-opus-4-5").unwrap().1);
        assert_eq!(t.lookup("claude-opus-4-1-20250805").unwrap().0, "claude-opus-4-1");
        assert_eq!(t.lookup("anthropic/claude-sonnet-4-5").unwrap().0, "claude-sonnet-4-5");
        assert_eq!(t.lookup("claude-sonnet-4-5[1m]").unwrap().0, "claude-sonnet-4-5");
        assert_eq!(t.lookup("openai/gpt-5.1-codex").unwrap().0, "gpt-5.1-codex");
        assert_eq!(t.lookup("gpt-5.1-codex-max-preview").unwrap().0, "gpt-5.1-codex-max");
        assert!(t.lookup("<synthetic>").is_none());
        assert!(t.lookup("totally-unknown").is_none());
        // a dotted version must not fall back to an older family member
        assert_ne!(t.lookup("gpt-5.9-codex").map(|x| x.0), Some("gpt-5"));
    }

    #[test]
    fn opus_5_5_is_not_priced_as_opus_4() {
        let p = *table().lookup("claude-opus-5-5").unwrap().1;
        assert_eq!(p.input, 4.0);
        assert_eq!(p.output, 20.0);
    }

    #[test]
    fn cost_math() {
        let p = Price {
            input: 3.0,
            output: 15.0,
            cache_read: Some(0.3),
            cache_write: Some(3.75),
            cache_write_1h: Some(6.0),
            ..Default::default()
        };
        let t = Tokens {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 1_000_000,
            cache_write_1h: 1_000_000,
            reasoning: 0,
        };
        assert!((p.cost(&t, false) - (3.0 + 15.0 + 0.3 + 3.75 + 6.0)).abs() < 1e-9);
        let fast = Price { fast_multiplier: Some(2.0), ..p };
        assert!((fast.cost(&t, true) - 2.0 * p.cost(&t, false)).abs() < 1e-9);
        assert!((p.cost(&t, true) - p.cost(&t, false)).abs() < 1e-9);
    }

    #[test]
    fn long_context_tier() {
        let p = Price {
            input: 1.25,
            output: 10.0,
            input_above_200k: Some(2.5),
            output_above_200k: Some(15.0),
            ..Default::default()
        };
        let short = Tokens { input: 100_000, output: 1_000, ..Default::default() };
        let long = Tokens { input: 300_000, output: 1_000, ..Default::default() };
        assert!((p.cost(&short, false) - (0.125 + 0.01)).abs() < 1e-9);
        assert!((p.cost(&long, false) - (0.75 + 0.015)).abs() < 1e-9);
    }
}
