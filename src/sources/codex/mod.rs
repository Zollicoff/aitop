//! Codex CLI: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`, and the
//! `rollout-*.jsonl.zst` files Codex compresses older sessions into.
//!
//! Each line is `{"timestamp", "type", "payload"}`. `session_meta` carries the
//! session id and cwd, `turn_context` the model, `thread_settings_applied`
//! the speed tier, and `event_msg` lines of type `token_count` the usage of
//! the request just made (`last_token_usage`), the session's running total
//! (`total_token_usage`) and the account's rate-limit windows.
//!
//! One record is produced per request, from `last_token_usage`, with the
//! running total used to check it. The rules follow T3 Code's reader
//! (`codexUsage.ts`) so that both programs count the same usage:
//!
//! * a forked session or sub-agent starts with a copy of its parent's
//!   history, which the parent's own log has already been billed for; the
//!   copy only sets the running total to continue from;
//! * a request is billed when the running total grew by exactly its usage
//!   (or the log starts or restarts there); a repeated or out-of-step
//!   snapshot is not a new request;
//! * newer logs also list requests as `token_usage_record` lines; those with
//!   a response id are billed directly, and the others only when a later
//!   snapshot's total accounts for them.

mod archive;
mod billing;
#[cfg(test)]
mod tests;
mod usage;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::Value;

use super::{discover, parse_ts, prune, Change, Ctx, Source, SourceStatus};
use crate::event::{Event, Tool};
use crate::limits::{LimitWindow, RateLimits};
use crate::paths::home;
use archive::is_compressed;
use billing::FileState;

pub struct CodexSource {
    roots: Vec<PathBuf>,
    files: HashMap<PathBuf, FileState>,
    limits: Option<RateLimits>,
    /// Where the records read from compressed logs are kept, so that a log
    /// is only decompressed once.
    archive_cache: Option<PathBuf>,
}

impl CodexSource {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        let archive_cache = Some(crate::paths::cache_dir().join("codex-zst"));
        CodexSource { roots, files: HashMap::new(), limits: None, archive_cache }
    }
}

fn is_log(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    name.ends_with(".jsonl") || is_compressed(path)
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

/// Just the payload's own `type`, to tell which payloads are worth reading.
#[derive(Deserialize)]
struct PayloadKind {
    #[serde(rename = "type")]
    kind: Option<String>,
}

impl Source for CodexSource {
    fn tool(&self) -> Tool {
        Tool::Codex
    }

    fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    fn add_roots(&mut self, roots: &[PathBuf]) {
        super::merge_roots(&mut self.roots, roots);
    }

    fn rate_limits(&self) -> Option<&RateLimits> {
        self.limits.as_ref()
    }

    fn scan(&mut self, ctx: &mut Ctx, out: &mut Vec<Event>) -> SourceStatus {
        let mut status = SourceStatus::new(Tool::Codex, &self.roots);
        let paths = ctx.recent(discover(&self.roots, is_log));
        status.files = paths.len();
        prune(&mut self.files, &paths);
        for path in &paths {
            let mut st = self.files.remove(path).unwrap_or_default();
            match st.tail.poll(path) {
                Ok(Change::Unchanged) => {}
                // A compressed log is complete: it is read whole, once.
                Ok(_) if is_compressed(path) => {
                    if let Err(e) = self.read_compressed(path, ctx, out) {
                        status.error(path, e);
                    }
                }
                Ok(change) => {
                    if change == Change::Reset {
                        // Rewritten file: forget what we derived from it.
                        st = FileState::restarted(st.tail);
                    }
                    let fallback_session: Arc<str> =
                        ctx.interner.get(&path.file_stem().unwrap_or_default().to_string_lossy());
                    let mut tail = std::mem::take(&mut st.tail);
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
    /// Read one line of a log: follow the session and add the requests it reports.
    fn handle_line(
        &mut self,
        line: &[u8],
        st: &mut FileState,
        fallback_session: &Arc<str>,
        ctx: &mut Ctx,
        out: &mut Vec<Event>,
    ) {
        let Ok(text) = std::str::from_utf8(line) else { return };
        // Most of a log is conversation; only these lines matter here.
        const WANTED: [&str; 5] =
            ["token_count", "session_meta", "turn_context", "token_usage_record", "thread_settings_applied"];
        if !WANTED.iter().any(|w| text.contains(w)) {
            return;
        }
        let Ok(l) = serde_json::from_str::<Line>(text) else { return };
        let Some(raw) = l.payload else { return };
        let ts = l.timestamp.as_deref().and_then(parse_ts);
        let payload = || serde_json::from_str::<Value>(raw.get()).ok().filter(Value::is_object);
        match l.kind.as_deref() {
            Some("session_meta") => {
                // Only the first one is this session's; a fork's log also holds its ancestors'.
                if !st.has_meta() {
                    if let Some(m) = payload() {
                        st.session_meta(&m, ts, ctx);
                    }
                }
                return;
            }
            Some("turn_context") => {
                if let Some(t) = payload() {
                    st.turn_context(&t, ctx);
                }
                return;
            }
            Some("token_usage_record") => {
                if let Some(p) = payload() {
                    st.usage_record(&p, ts, fallback_session, ctx, out);
                }
                return;
            }
            _ => {}
        }
        // The rest are told apart by the payload's own type.
        let Ok(kind) = serde_json::from_str::<PayloadKind>(raw.get()) else { return };
        match kind.kind.as_deref() {
            Some("thread_settings_applied") => {
                if let Some(p) = payload() {
                    st.settings(&p);
                }
            }
            Some("token_count") => {
                let Some(p) = payload() else { return };
                if let (Some(ts), Some(rl)) = (ts, p.get("rate_limits").filter(|r| !r.is_null())) {
                    let snap = parse_limits(rl, ts);
                    if (snap.primary.is_some() || snap.secondary.is_some())
                        && self.limits.as_ref().is_none_or(|l| l.ts <= ts)
                    {
                        self.limits = Some(snap);
                    }
                }
                st.token_count(&p, ts, fallback_session, ctx, out);
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
