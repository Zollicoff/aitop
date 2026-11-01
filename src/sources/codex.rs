//! Codex CLI: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
//!
//! Each line is `{"timestamp", "type", "payload"}`. `session_meta` carries the
//! session id and cwd, `turn_context` the model, and `event_msg` lines of
//! type `token_count` carry the *cumulative* usage for the session plus the
//! account's rate-limit windows. Per-turn usage is the difference between
//! consecutive cumulative totals.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::value::RawValue;

use super::{discover, parse_ts, prune, Change, Ctx, Source, SourceStatus, Tail};
use crate::event::{Event, Tokens, Tool};
use crate::limits::{LimitWindow, RateLimits};
use crate::paths::home;

#[derive(Default)]
struct FileState {
    tail: Tail,
    session: Option<Arc<str>>,
    project: Option<(Arc<str>, Arc<str>)>,
    model: Option<Arc<str>>,
    prev: Option<Usage>,
}

pub struct CodexSource {
    roots: Vec<PathBuf>,
    files: HashMap<PathBuf, FileState>,
    /// Highest cumulative usage seen per session, so a resumed session that
    /// continues in a new rollout file is not counted twice.
    sessions: HashMap<Arc<str>, Usage>,
    limits: Option<RateLimits>,
}

impl CodexSource {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        CodexSource { roots, files: HashMap::new(), sessions: HashMap::new(), limits: None }
    }
}

/// `$CODEX_HOME` (default `~/.codex`): live and archived sessions.
pub fn default_roots() -> Vec<PathBuf> {
    let base = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"));
    vec![base.join("sessions"), base.join("archived_sessions")]
}

#[derive(Deserialize)]
struct Line<'a> {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(borrow)]
    payload: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct SessionMeta {
    id: Option<String>,
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct TurnContext {
    cwd: Option<String>,
    model: Option<String>,
}

#[derive(Deserialize)]
struct EventMsg {
    #[serde(rename = "type")]
    kind: Option<String>,
    info: Option<Info>,
    rate_limits: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Info {
    total_token_usage: Option<Usage>,
    last_token_usage: Option<Usage>,
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(default)]
struct Usage {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
}

impl Usage {
    fn total(&self) -> u64 {
        if self.total_tokens > 0 {
            self.total_tokens
        } else {
            self.input_tokens + self.output_tokens
        }
    }

    /// `self - prev`, or `None` if the counter went backwards.
    fn minus(&self, prev: &Usage) -> Option<Usage> {
        if self.total() < prev.total()
            || self.input_tokens < prev.input_tokens
            || self.output_tokens < prev.output_tokens
        {
            return None;
        }
        Some(Usage {
            input_tokens: self.input_tokens - prev.input_tokens,
            cached_input_tokens: self.cached_input_tokens.saturating_sub(prev.cached_input_tokens),
            output_tokens: self.output_tokens - prev.output_tokens,
            reasoning_output_tokens: self.reasoning_output_tokens.saturating_sub(prev.reasoning_output_tokens),
            total_tokens: self.total() - prev.total(),
        })
    }
}

impl Source for CodexSource {
    fn tool(&self) -> Tool {
        Tool::Codex
    }

    fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    fn rate_limits(&self) -> Option<&RateLimits> {
        self.limits.as_ref()
    }

    fn scan(&mut self, ctx: &mut Ctx, out: &mut Vec<Event>) -> SourceStatus {
        let mut status = SourceStatus::new(Tool::Codex, &self.roots);
        // Rollout names start with a timestamp, so this is chronological,
        // which the resumed-session accounting relies on.
        let paths = discover(&self.roots, |p| p.extension().is_some_and(|e| e == "jsonl"));
        status.files = paths.len();
        prune(&mut self.files, &paths);
        for path in &paths {
            let mut st = self.files.remove(path).unwrap_or_default();
            match st.tail.poll(path) {
                Ok(Change::Unchanged) => {}
                Ok(change) => {
                    if change == Change::Reset {
                        // Rewritten file: forget what we derived from it.
                        st = FileState { tail: st.tail, ..Default::default() };
                    }
                    let fallback_session: Arc<str> =
                        ctx.interner.get(&path.file_stem().unwrap_or_default().to_string_lossy());
                    let mut tail = st.tail;
                    let res =
                        tail.read_lines(path, |line| self.handle_line(line, &mut st, &fallback_session, ctx, out));
                    st.tail = tail;
                    if let Err(e) = res {
                        status.error(path, e);
                    }
                }
                Err(e) => status.error(path, e),
            }
            self.files.insert(path.clone(), st);
        }
        status
    }
}

impl CodexSource {
    fn handle_line(
        &mut self,
        line: &[u8],
        st: &mut FileState,
        fallback_session: &Arc<str>,
        ctx: &mut Ctx,
        out: &mut Vec<Event>,
    ) {
        let Ok(text) = std::str::from_utf8(line) else { return };
        if !(text.contains("token_count") || text.contains("session_meta") || text.contains("turn_context")) {
            return;
        }
        let Ok(l) = serde_json::from_str::<Line>(text) else { return };
        let Some(payload) = l.payload else { return };
        let ts = l.timestamp.as_deref().and_then(parse_ts);
        match l.kind.as_deref() {
            Some("session_meta") => {
                if let Ok(m) = serde_json::from_str::<SessionMeta>(payload.get()) {
                    if let Some(id) = m.id {
                        st.session = Some(ctx.interner.get(&id));
                    }
                    if let Some(cwd) = m.cwd {
                        st.project = Some(ctx.projects.resolve(&cwd, ctx.interner));
                    }
                }
            }
            Some("turn_context") => {
                if let Ok(t) = serde_json::from_str::<TurnContext>(payload.get()) {
                    if let Some(m) = t.model {
                        st.model = Some(ctx.interner.get(&m));
                    }
                    if let Some(cwd) = t.cwd {
                        st.project = Some(ctx.projects.resolve(&cwd, ctx.interner));
                    }
                }
            }
            Some("event_msg") => {
                let Ok(m) = serde_json::from_str::<EventMsg>(payload.get()) else { return };
                if m.kind.as_deref() != Some("token_count") {
                    return;
                }
                let Some(ts) = ts else { return };
                if let Some(rl) = &m.rate_limits {
                    let snap = parse_limits(rl, ts);
                    if (snap.primary.is_some() || snap.secondary.is_some())
                        && self.limits.as_ref().is_none_or(|l| l.ts <= ts)
                    {
                        self.limits = Some(snap);
                    }
                }
                let Some(info) = m.info else { return };
                let Some(total) = info.total_token_usage else { return };
                let session = st.session.clone().unwrap_or_else(|| fallback_session.clone());
                let baseline = match (st.prev, self.sessions.get(&session)) {
                    (Some(p), _) => Some(p),
                    // First count in this file: continue from what earlier files of the same session reported.
                    (None, Some(s)) if total.total() >= s.total() => Some(*s),
                    _ => None,
                };
                let delta = match baseline {
                    Some(b) => total.minus(&b).or(info.last_token_usage),
                    None => Some(total),
                };
                st.prev = Some(total);
                let best = self.sessions.entry(session.clone()).or_default();
                if total.total() >= best.total() {
                    *best = total;
                }
                let Some(d) = delta else { return };
                if d.total() == 0 {
                    return;
                }
                let cached = d.cached_input_tokens.min(d.input_tokens);
                let tokens = Tokens {
                    input: d.input_tokens - cached,
                    output: d.output_tokens,
                    cache_read: cached,
                    cache_write: 0,
                    cache_write_1h: 0,
                    reasoning: d.reasoning_output_tokens,
                };
                let (project, project_path) =
                    st.project.clone().unwrap_or_else(|| (ctx.interner.get("unknown"), ctx.interner.get("unknown")));
                let id = format!("x:{session}:{}", total.total());
                out.push(Event {
                    tool: Tool::Codex,
                    id: ctx.interner.get(&id),
                    ts,
                    session,
                    project,
                    project_path,
                    model: st.model.clone().unwrap_or_else(|| ctx.interner.get("unknown")),
                    tokens,
                    fast: false,
                    cost: 0.0,
                    priced: false,
                });
            }
            _ => {}
        }
    }
}

fn parse_limits(v: &serde_json::Value, ts: i64) -> RateLimits {
    let window = |w: &serde_json::Value| -> Option<LimitWindow> {
        let used = w.get("used_percent")?.as_f64()?;
        let resets_at = w
            .get("resets_at")
            .and_then(|x| x.as_i64())
            .map(|s| s * 1000)
            .or_else(|| w.get("resets_in_seconds").and_then(|x| x.as_i64()).map(|s| ts + s * 1000));
        Some(LimitWindow {
            used_percent: used,
            window_minutes: w.get("window_minutes").and_then(|x| x.as_u64()),
            resets_at,
        })
    };
    RateLimits {
        ts,
        primary: v.get("primary").and_then(window),
        secondary: v.get("secondary").and_then(window),
        plan: v.get("plan_type").and_then(|p| p.as_str()).map(String::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Interner;
    use crate::project::ProjectResolver;

    fn tc(ts: &str, input: u64, cached: u64, output: u64, reasoning: u64, limits: &str) -> String {
        let total = input + output;
        format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output},"reasoning_output_tokens":{reasoning},"total_tokens":{total}}},"last_token_usage":null,"model_context_window":272000}},"rate_limits":{limits}}}}}"#
        )
    }

    #[test]
    fn cumulative_counts_become_deltas() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("2026/11/01");
        std::fs::create_dir_all(&day).unwrap();
        let limits = r#"{"primary":{"used_percent":34.0,"window_minutes":300,"resets_in_seconds":600},"secondary":{"used_percent":12.5,"window_minutes":10080,"resets_at":1800000000}}"#;
        let lines = [
            r#"{"timestamp":"2026-11-01T10:00:00Z","type":"session_meta","payload":{"id":"sess-1","cwd":"/nowhere/webapp","cli_version":"0.50.0"}}"#.to_string(),
            r#"{"timestamp":"2026-11-01T10:00:01Z","type":"turn_context","payload":{"cwd":"/nowhere/webapp","model":"gpt-5-codex"}}"#.to_string(),
            r#"{"timestamp":"2026-11-01T10:00:02Z","type":"response_item","payload":{"type":"message","content":[{"type":"output_text","text":"token_count"}]}}"#.to_string(),
            tc("2026-11-01T10:00:05Z", 1000, 200, 100, 40, "null"),
            tc("2026-11-01T10:00:06Z", 1000, 200, 100, 40, "null"), // duplicate snapshot
            tc("2026-11-01T10:01:00Z", 3000, 1200, 300, 90, limits),
        ];
        std::fs::write(day.join("rollout-2026-11-01T10-00-00-sess-1.jsonl"), lines.join("\n") + "\n").unwrap();

        let mut src = CodexSource::new(vec![dir.path().to_path_buf()]);
        let mut interner = Interner::default();
        let mut projects = ProjectResolver::new();
        let mut ctx = Ctx { interner: &mut interner, projects: &mut projects };
        let mut out = Vec::new();
        src.scan(&mut ctx, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].tokens,
            Tokens { input: 800, output: 100, cache_read: 200, cache_write: 0, cache_write_1h: 0, reasoning: 40 }
        );
        assert_eq!(
            out[1].tokens,
            Tokens { input: 1000, output: 200, cache_read: 1000, cache_write: 0, cache_write_1h: 0, reasoning: 50 }
        );
        assert_eq!(&*out[0].model, "gpt-5-codex");
        assert_eq!(&*out[0].project, "webapp");
        assert_eq!(&*out[0].session, "sess-1");
        let l = src.rate_limits().cloned().unwrap();
        assert_eq!(l.primary.as_ref().unwrap().used_percent, 34.0);
        assert_eq!(l.primary.as_ref().unwrap().resets_at, Some(out[1].ts + 600_000));
        assert_eq!(l.secondary.as_ref().unwrap().resets_at, Some(1_800_000_000_000));

        // A resumed session continuing in a new file only counts the new part.
        let resumed = [
            r#"{"timestamp":"2026-11-01T12:00:00Z","type":"session_meta","payload":{"id":"sess-1","cwd":"/nowhere/webapp"}}"#.to_string(),
            tc("2026-11-01T12:00:05Z", 3500, 1200, 350, 90, "null"),
        ];
        std::fs::write(day.join("rollout-2026-11-01T12-00-00-sess-1.jsonl"), resumed.join("\n") + "\n").unwrap();
        out.clear();
        src.scan(&mut ctx, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tokens.input + out[0].tokens.cache_read, 500);
        assert_eq!(out[0].tokens.output, 50);
    }
}
