//! Reading the answer to T3 Code's `server.getUsageSummary`.
//!
//! The summary has `buckets` (one per day, provider and model, sometimes
//! split further by transcript folder) and `sources` (the folders it read,
//! each with the machine they are on). aitop takes the folders of the tools
//! it has a panel for, and adds the buckets up into one total per tool,
//! model and day.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

use crate::event::{Tokens, Tool};

/// A transcript folder T3 reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub tool: Tool,
    /// What T3 calls the machine the folder is on.
    pub host: String,
    pub dir: PathBuf,
    /// False when T3 found no such folder.
    pub present: bool,
}

/// A machine's usage of one model on one day.
#[derive(Debug, Clone, PartialEq)]
pub struct Total {
    /// What T3 calls the machine the usage was read on (empty if it did not say).
    pub host: String,
    pub tool: Tool,
    pub model: String,
    /// `YYYY-MM-DD`, in the time zone the summary was asked for.
    pub day: String,
    pub tokens: Tokens,
    pub responses: u32,
    pub sessions: u32,
}

#[derive(Debug, Default, PartialEq)]
pub struct Summary {
    pub sources: Vec<Source>,
    pub totals: Vec<Total>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bucket {
    day: String,
    provider: String,
    model: String,
    /// The transcript folder the bucket was read from, when the server splits by folder.
    #[serde(default)]
    source_path: Option<String>,
    #[serde(default)]
    totals: Counts,
    #[serde(default)]
    records: u64,
    #[serde(default)]
    sessions: u64,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Counts {
    uncached_input_tokens: u64,
    cached_input_tokens: u64,
    cache_creation_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSource {
    fingerprint: Fingerprint,
    #[serde(default)]
    status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fingerprint {
    host_id: String,
    provider: String,
    resolved_home_path: String,
}

/// The tool a T3 provider name stands for, if aitop has a panel for it.
fn tool(provider: &str) -> Option<Tool> {
    match provider {
        "claude" => Some(Tool::Claude),
        "codex" => Some(Tool::Codex),
        _ => None,
    }
}

/// Entries T3 added in a newer version than this code knows are skipped
/// rather than failing the whole summary.
fn each<T: for<'de> Deserialize<'de>>(list: &Value) -> impl Iterator<Item = T> + '_ {
    list.as_array().into_iter().flatten().filter_map(|v| T::deserialize(v).ok())
}

pub fn parse(summary: &Value) -> Summary {
    let sources: Vec<Source> = each::<RawSource>(&summary["sources"])
        .filter_map(|s| {
            Some(Source {
                tool: tool(&s.fingerprint.provider)?,
                host: s.fingerprint.host_id,
                dir: PathBuf::from(s.fingerprint.resolved_home_path),
                present: s.status != "missing",
            })
        })
        .collect();

    // A bucket belongs to the machine its folder is on; a summary that names
    // no folders is all from the one machine its sources are on.
    let only_host = match sources.as_slice() {
        [first, rest @ ..] if rest.iter().all(|s| s.host == first.host) => first.host.clone(),
        _ => String::new(),
    };
    let mut totals: BTreeMap<(String, usize, String, String), Total> = BTreeMap::new();
    for b in each::<Bucket>(&summary["buckets"]) {
        let Some(tool) = tool(&b.provider) else { continue };
        let host = b
            .source_path
            .as_deref()
            .and_then(|path| sources.iter().find(|s| s.tool == tool && s.dir.as_os_str() == path))
            .map_or_else(|| only_host.clone(), |s| s.host.clone());
        let key = (host.clone(), tool.index(), b.day.clone(), b.model.clone());
        let t = totals.entry(key).or_insert_with(|| Total {
            host,
            tool,
            model: b.model,
            day: b.day,
            tokens: Tokens::default(),
            responses: 0,
            sessions: 0,
        });
        t.tokens += Tokens {
            input: b.totals.uncached_input_tokens,
            output: b.totals.output_tokens,
            cache_read: b.totals.cached_input_tokens,
            cache_write: b.totals.cache_creation_tokens,
            cache_write_1h: 0,
            reasoning: b.totals.reasoning_tokens,
        };
        t.responses = t.responses.saturating_add(b.records.min(u64::from(u32::MAX)) as u32);
        t.sessions = t.sessions.saturating_add(b.sessions.min(u64::from(u32::MAX)) as u32);
    }
    Summary { sources, totals: totals.into_values().filter(|t| !t.tokens.is_zero()).collect() }
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;

    /// A summary shaped like a real one (trimmed), for `host`.
    pub(crate) fn sample(host: &str) -> Value {
        let bucket = |day: &str, provider: &str, model: &str, path: &str, input: u64, records: u64| {
            json!({
                "day": day, "provider": provider, "model": model, "sourcePath": path,
                "totals": {"uncachedInputTokens": input, "cachedInputTokens": 354808, "cacheCreationTokens": 79140,
                           "outputTokens": 5656, "reasoningTokens": 12},
                "costUsd": 0.0222, "cacheSavingsUsd": 0.1, "costSource": "modelPriced",
                "records": records, "unpricedRecords": 0, "sessions": 2
            })
        };
        let source = |provider: &str, dir: &str, status: &str| {
            json!({"fingerprint": {"hostId": host, "provider": provider, "resolvedHomePath": dir, "volumeId": "1:2"},
                   "status": status, "scannedFiles": 3, "skippedFiles": 0, "malformedRecords": 0,
                   "distinctSessions": 4, "message": null})
        };
        json!({
            "contractVersion": 6, "readAt": "2026-11-21T04:25:50.871Z", "timeZone": "UTC",
            "sinceDay": "2026-11-14", "untilDay": "2026-11-20",
            "buckets": [
                bucket("2026-11-14", "claude", "claude-haiku-5-5", "/home/z/.claude/projects", 34, 17),
                bucket("2026-11-14", "claude", "claude-haiku-5-5", "/home/z/.claude-alt/projects", 6, 3),
                bucket("2026-11-15", "codex", "gpt-6-astra", "/home/z/.codex/sessions", 100, 9),
                bucket("2026-11-15", "grok", "grok-build", "/home/z/.grok/sessions", 50, 1),
                {"day": "2026-11-16", "provider": "claude"},
                "a future kind of entry"
            ],
            "sources": [
                source("claude", "/home/z/.claude/projects", "ok"),
                source("claude", "/home/z/.claude-alt/projects", "partial"),
                source("codex", "/home/z/.codex/sessions", "ok"),
                source("pi", "/home/z/.pi/agent/sessions", "missing"),
                source("claude", "/home/z/.config/claude/projects", "missing"),
                {"fingerprint": {"hostId": host}}
            ],
            "pricing": {"status": "fresh", "source": "x", "fetchedAt": null, "knownModels": 1},
            "scanDurationMs": 9
        })
    }

    #[test]
    fn parses_a_summary() {
        let s = parse(&sample("studio-pc"));
        // The two folders' haiku buckets for the 14th are one total; Grok has no panel.
        assert_eq!(s.totals.len(), 2);
        let haiku = &s.totals[0];
        assert_eq!(
            (haiku.tool, haiku.model.as_str(), haiku.day.as_str(), haiku.host.as_str()),
            (Tool::Claude, "claude-haiku-5-5", "2026-11-14", "studio-pc")
        );
        assert_eq!(
            haiku.tokens,
            Tokens {
                input: 40,
                output: 11312,
                cache_read: 709616,
                cache_write: 158280,
                cache_write_1h: 0,
                reasoning: 24
            }
        );
        assert_eq!((haiku.responses, haiku.sessions), (20, 4));
        assert_eq!((s.totals[1].tool, s.totals[1].responses), (Tool::Codex, 9));

        let dirs: Vec<(&str, bool)> = s.sources.iter().map(|x| (x.dir.to_str().unwrap(), x.present)).collect();
        assert_eq!(
            dirs,
            [
                ("/home/z/.claude/projects", true),
                ("/home/z/.claude-alt/projects", true),
                ("/home/z/.codex/sessions", true),
                ("/home/z/.config/claude/projects", false)
            ]
        );
        assert!(s.sources.iter().all(|x| x.host == "studio-pc"));
        assert_eq!(parse(&json!({})), Summary::default());

        // Folders on two machines (a shared home, say): each bucket goes to its folder's machine.
        let mut shared = sample("laptop");
        shared["sources"][1]["fingerprint"]["hostId"] = json!("desk");
        let hosts: Vec<(String, u32)> = parse(&shared).totals.into_iter().map(|t| (t.host, t.responses)).collect();
        assert_eq!(hosts, [("desk".to_string(), 3), ("laptop".to_string(), 17), ("laptop".to_string(), 9)]);
    }
}
