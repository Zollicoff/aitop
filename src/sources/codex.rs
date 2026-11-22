//! Codex CLI: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::Value;

use super::{discover, parse_ts, prune, Change, Ctx, Source, SourceStatus, Tail};
use crate::event::{Event, Speed, Tokens, Tool};
use crate::limits::{LimitWindow, RateLimits};
use crate::paths::home;

/// A fork copies and restamps its parent's history in a burst; the child's
/// own first request comes seconds later.
const FORK_COPY_MAX_GAP_MS: i64 = 1000;

/// Token counters as Codex reports them: `input` includes the cached and
/// cache-write parts, `output` includes reasoning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
struct Usage {
    input: u64,
    cached: u64,
    cache_write: u64,
    output: u64,
    reasoning: u64,
    total: u64,
}

impl Usage {
    fn read(v: Option<&Value>) -> Option<Usage> {
        let o = v?.as_object()?;
        // A counter that is present must be a whole number.
        let get = |k: &str, default: u64| match o.get(k) {
            None => Some(default),
            Some(x) => x.as_u64(),
        };
        let input = o.get("input_tokens")?.as_u64()?;
        let output = o.get("output_tokens")?.as_u64()?;
        let u = Usage {
            input,
            cached: get("cached_input_tokens", 0)?,
            cache_write: get("cache_write_input_tokens", 0)?,
            output,
            reasoning: get("reasoning_output_tokens", 0)?,
            total: get("total_tokens", input + output)?,
        };
        u.valid().then_some(u)
    }

    fn valid(&self) -> bool {
        self.cached + self.cache_write <= self.input
            && self.reasoning <= self.output
            && self.total >= self.input + self.output
    }

    fn fields(&self) -> [u64; 6] {
        [self.input, self.cached, self.cache_write, self.output, self.reasoning, self.total]
    }

    fn from_fields(f: [u64; 6]) -> Option<Usage> {
        let u = Usage { input: f[0], cached: f[1], cache_write: f[2], output: f[3], reasoning: f[4], total: f[5] };
        u.valid().then_some(u)
    }

    fn plus(&self, o: &Usage) -> Option<Usage> {
        let (a, b) = (self.fields(), o.fields());
        Usage::from_fields(std::array::from_fn(|i| a[i] + b[i]))
    }

    /// `self - o`, if no counter goes below zero and what is left is usage.
    fn minus(&self, o: &Usage) -> Option<Usage> {
        let (a, b) = (self.fields(), o.fields());
        let mut out = [0; 6];
        for i in 0..6 {
            out[i] = a[i].checked_sub(b[i])?;
        }
        Usage::from_fields(out)
    }

    /// Every counter of `self` is exactly `o` more than `prev`'s.
    fn is_step_from(&self, prev: &Usage, o: &Usage) -> bool {
        let (a, p, b) = (self.fields(), prev.fields(), o.fields());
        (0..6).all(|i| a[i].checked_sub(p[i]) == Some(b[i]))
    }

    fn below(&self, o: &Usage) -> bool {
        self.fields().iter().zip(o.fields()).any(|(a, b)| *a < b)
    }

    /// Nothing was sent or generated: a context-size marker, not a request.
    fn is_empty(&self) -> bool {
        self.input + self.output == 0
    }
}

/// A request listed without a response id, waiting for a snapshot to vouch for it.
#[derive(Clone, Debug)]
struct Pending {
    ts: i64,
    model: Arc<str>,
    speed: Speed,
    usage: Usage,
}

#[derive(Default)]
struct FileState {
    tail: Tail,
    saw_meta: bool,
    session: Option<Arc<str>>,
    project: Option<(Arc<str>, Arc<str>)>,
    model: Option<Arc<str>>,
    speed: Speed,
    /// Still inside the history a fork copied from its parent.
    fork_copies: bool,
    fork_anchor: i64,
    /// The last request billed from a snapshot.
    last_billed: Option<Usage>,
    /// The running total to measure the next request against, and the
    /// latest time it was set.
    baseline: Option<Usage>,
    baseline_at: Option<i64>,
    /// The snapshot at which the running total last restarted.
    restart: Option<(i64, Usage)>,
    pending: Option<Vec<Pending>>,
    /// Requests are being billed from `token_usage_record` lines.
    ledger: bool,
    /// How many requests with the same time and usage were seen, for their ids.
    repeats: HashMap<(i64, Usage), u32>,
}

impl FileState {
    fn date_baseline(&mut self, ts: i64) {
        self.baseline_at = Some(self.baseline_at.map_or(ts, |t| t.max(ts)));
    }

    fn rebase(&mut self, total: Usage, ts: i64) {
        self.baseline = Some(total);
        self.date_baseline(ts);
        self.pending = None;
        if total.is_empty() {
            self.restart = None;
        }
    }

    fn advance(&mut self, by: &Usage) {
        self.baseline = self.baseline.unwrap_or_default().plus(by);
    }

    /// Whether a usage line at `ts` is part of the copied history. The first
    /// one past the burst ends the copy for the rest of the file.
    fn is_fork_copy(&mut self, ts: i64) -> bool {
        if !self.fork_copies {
            return false;
        }
        if ts - self.fork_anchor < FORK_COPY_MAX_GAP_MS {
            self.fork_anchor = ts;
            return true;
        }
        self.fork_copies = false;
        self.last_billed = None;
        false
    }
}

pub struct CodexSource {
    roots: Vec<PathBuf>,
    files: HashMap<PathBuf, FileState>,
    limits: Option<RateLimits>,
}

impl CodexSource {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        CodexSource { roots, files: HashMap::new(), limits: None }
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
        let paths = ctx.recent(discover(&self.roots, |p| p.extension().is_some_and(|e| e == "jsonl")));
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

/// A forked session or a sub-agent: its log opens with its parent's history.
fn is_fork(meta: &Value) -> bool {
    meta["forked_from_id"].is_string() || meta["source"]["subagent"]["thread_spawn"]["parent_thread_id"].is_string()
}

/// Missing, default and standard tiers are standard; "fast" is another name for priority.
fn speed(service_tier: &Value) -> Speed {
    match service_tier.as_str() {
        Some("priority" | "fast") => Speed::Fast,
        Some("ultrafast") => Speed::Ultrafast,
        _ => Speed::Standard,
    }
}

/// The requests listed since the last snapshot, if together they account
/// for exactly how far the running total moved. `last` is the final one and
/// is billed by the caller.
fn bridged(previous: Option<Usage>, total: &Usage, last: &Usage, pending: Option<&[Pending]>) -> Option<Vec<Pending>> {
    let (previous, pending) = (previous?, pending?);
    let (fin, earlier) = pending.split_last()?;
    let listed = pending.iter().try_fold(Usage::default(), |sum, p| {
        let (a, b) = (sum.fields(), p.usage.fields());
        // Summed counter by counter; the sum itself need not be valid usage.
        Some(Usage {
            input: a[0] + b[0],
            cached: a[1] + b[1],
            cache_write: a[2] + b[2],
            output: a[3] + b[3],
            reasoning: a[4] + b[4],
            total: a[5] + b[5],
        })
    })?;
    if !total.fields().iter().zip(previous.fields()).zip(listed.fields()).all(|((t, p), l)| t.checked_sub(p) == Some(l))
    {
        return None;
    }
    let rest = fin.usage.minus(last)?;
    let mut out = earlier.to_vec();
    out.push(Pending { usage: rest, ..fin.clone() });
    out.retain(|p| !p.usage.is_empty());
    Some(out)
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
                if st.saw_meta {
                    return;
                }
                let Some(m) = payload() else { return };
                st.saw_meta = true;
                if let Some(id) = m["id"].as_str().or(m["session_id"].as_str()) {
                    st.session = Some(ctx.interner.get(id));
                }
                if let Some(cwd) = m["cwd"].as_str() {
                    st.project = Some(ctx.projects.resolve(cwd, ctx.interner));
                }
                if let (Some(ts), true) = (ts, is_fork(&m)) {
                    st.fork_copies = true;
                    st.fork_anchor = ts;
                }
                return;
            }
            Some("turn_context") => {
                let Some(t) = payload() else { return };
                st.ledger = false;
                if let Some(m) = t["model"].as_str() {
                    st.model = Some(ctx.interner.get(m));
                }
                if let Some(cwd) = t["cwd"].as_str() {
                    st.project = Some(ctx.projects.resolve(cwd, ctx.interner));
                }
                return;
            }
            Some("token_usage_record") => {
                let Some(p) = payload() else { return };
                self.usage_record(&p, ts, st, fallback_session, ctx, out);
                return;
            }
            _ => {}
        }
        let Ok(kind) = serde_json::from_str::<PayloadKind>(raw.get()) else { return };
        match kind.kind.as_deref() {
            Some("thread_settings_applied") => {
                if let Some(settings) = payload().as_ref().map(|p| &p["thread_settings"]).filter(|s| s.is_object()) {
                    st.speed = speed(&settings["service_tier"]);
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
                self.token_count(&p, ts, st, fallback_session, ctx, out);
            }
            _ => {}
        }
    }

    /// A `token_usage_record` line: one request, as newer logs list them.
    fn usage_record(
        &mut self,
        p: &Value,
        ts: Option<i64>,
        st: &mut FileState,
        fallback_session: &Arc<str>,
        ctx: &mut Ctx,
        out: &mut Vec<Event>,
    ) {
        let (Some(ts), Some(usage), Some(model)) = (ts, Usage::read(p.get("usage")), st.model.clone()) else { return };
        if st.is_fork_copy(ts) {
            return;
        }
        let this = Pending { ts, model, speed: st.speed, usage };
        let Some(response_id) = p["response_id"].as_str().filter(|r| !r.is_empty()) else {
            st.ledger = false;
            st.pending.get_or_insert_default().push(this);
            return;
        };
        // An identified request stands on its own, and vouches for the ones before it.
        st.ledger = true;
        for earlier in st.pending.take().unwrap_or_default() {
            st.advance(&earlier.usage);
            emit(st, &earlier, None, fallback_session, ctx, out);
        }
        st.advance(&usage);
        st.date_baseline(ts);
        emit(st, &this, Some(response_id), fallback_session, ctx, out);
    }

    /// A `token_count` snapshot: the request just made and the running total.
    fn token_count(
        &mut self,
        p: &Value,
        ts: Option<i64>,
        st: &mut FileState,
        fallback_session: &Arc<str>,
        ctx: &mut Ctx,
        out: &mut Vec<Event>,
    ) {
        let Some(info) = p.get("info").filter(|i| i.is_object()) else { return };
        let (Some(ts), Some(model)) = (ts, st.model.clone()) else { return };
        let last = Usage::read(info.get("last_token_usage"));
        let has_total = info.get("total_token_usage").is_some_and(|t| !t.is_null());
        let total = if has_total { Usage::read(info.get("total_token_usage")) } else { None };

        if st.is_fork_copy(ts) {
            // Billed in the parent's log; here it only says where the total stands.
            if let Some(t) = total {
                st.rebase(t, ts);
            } else if let (false, Some(l)) = (has_total, last.filter(|l| Some(*l) != st.last_billed)) {
                st.advance(&l);
                st.date_baseline(ts);
            }
            st.last_billed = last;
            return;
        }
        if st.ledger {
            if let Some(t) = total {
                st.rebase(t, ts);
            }
            return;
        }

        let (previous, previous_at, pending) = (st.baseline, st.baseline_at, st.pending.clone());
        // A valid total is the next baseline even if its request turns out not to be billable.
        if let Some(t) = total {
            st.rebase(t, ts);
        }
        let Some(last) = last.filter(|_| !has_total || total.is_some()) else { return };
        if last.is_empty() {
            st.last_billed = None;
            return;
        }
        if total.is_some() && total == previous {
            return;
        }
        if last.total != last.input + last.output {
            return;
        }

        let mut unreported = Vec::new();
        if let Some(total) = total {
            // A session resumed in a new Codex process starts its total again at this request.
            let restarted = total == last;
            if restarted && st.restart == Some((ts, last)) {
                // The same event again.
                st.baseline = previous;
                if previous_at.is_none() {
                    st.baseline_at = None;
                }
                return;
            }
            let lowered = previous.is_some_and(|p| total.below(&p));
            let rewound = lowered && previous_at.is_some_and(|at| ts > at);
            let reconciled = match previous {
                Some(p) if !rewound => total.is_step_from(&p, &last),
                // Nothing to measure from: the total must at least contain the request.
                _ => total.minus(&last).is_some(),
            };
            if !restarted && !reconciled {
                let Some(listed) = bridged(previous, &total, &last, pending.as_deref()) else {
                    // An old snapshot written again is not the next baseline.
                    let replayed = previous_at.is_some_and(|at| ts < at || (ts == at && lowered && !total.is_empty()));
                    if replayed {
                        st.baseline = previous;
                        if pending.is_some() {
                            st.pending = pending;
                        }
                    }
                    return;
                };
                unreported = listed;
            }
            if restarted {
                st.restart = Some((ts, last));
            }
        } else {
            if Some(last) == st.last_billed {
                return;
            }
            let Some(advanced) = previous.unwrap_or_default().plus(&last) else { return };
            st.baseline = Some(advanced);
            st.date_baseline(ts);
        }
        st.last_billed = Some(last);

        for earlier in &unreported {
            emit(st, earlier, None, fallback_session, ctx, out);
        }
        emit(st, &Pending { ts, model, speed: st.speed, usage: last }, None, fallback_session, ctx, out);
    }
}

/// Add one request to `out`.
fn emit(
    st: &mut FileState,
    r: &Pending,
    response_id: Option<&str>,
    fallback_session: &Arc<str>,
    ctx: &mut Ctx,
    out: &mut Vec<Event>,
) {
    let u = r.usage;
    let tokens = Tokens {
        input: u.input - u.cached - u.cache_write,
        output: u.output,
        cache_read: u.cached,
        cache_write: u.cache_write,
        cache_write_1h: 0,
        reasoning: u.reasoning,
    };
    if tokens.is_zero() {
        return;
    }
    let session = st.session.clone().unwrap_or_else(|| fallback_session.clone());
    // The same request read from two files (a moved or copied log) gets the
    // same id; equal requests within a file are told apart by their order.
    let id = match response_id {
        Some(id) => format!("x2:{session}:r:{id}"),
        None => {
            let nth = st.repeats.entry((r.ts, u)).or_default();
            *nth += 1;
            format!("x2:{session}:{}:{}.{}.{}:{nth}", r.ts, u.input, u.cached, u.output)
        }
    };
    let (project, project_path) =
        st.project.clone().unwrap_or_else(|| (ctx.interner.get("unknown"), ctx.interner.get("unknown")));
    out.push(Event {
        tool: Tool::Codex,
        host: ctx.host.clone(),
        id: ctx.interner.get(&id),
        ts: r.ts,
        session,
        project,
        project_path,
        model: r.model.clone(),
        tokens,
        speed: r.speed,
        cost: 0.0,
        priced: false,
        rollup: None,
    });
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

    const META: &str = r#"{"timestamp":"2026-11-01T10:00:00Z","type":"session_meta","payload":{"id":"sess-1","cwd":"/nowhere/webapp","cli_version":"0.50.0"}}"#;
    const TURN: &str = r#"{"timestamp":"2026-11-01T10:00:01Z","type":"turn_context","payload":{"cwd":"/nowhere/webapp","model":"gpt-5-codex"}}"#;

    /// A `token_count` line: this request's `(input, cached, output, reasoning)` and the running total's.
    fn tc(ts: &str, last: (u64, u64, u64, u64), total: (u64, u64, u64, u64), limits: &str) -> String {
        let usage = |(input, cached, output, reasoning): (u64, u64, u64, u64)| {
            format!(
                r#"{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output},"reasoning_output_tokens":{reasoning},"total_tokens":{}}}"#,
                input + output
            )
        };
        format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{},"last_token_usage":{},"model_context_window":272000}},"rate_limits":{limits}}}}}"#,
            usage(total),
            usage(last)
        )
    }

    /// Scan `files` (name, lines) from a fresh source.
    fn scan(files: &[(&str, Vec<String>)]) -> (Vec<Event>, CodexSource, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("2026/11/01");
        std::fs::create_dir_all(&day).unwrap();
        for (name, lines) in files {
            std::fs::write(day.join(name), lines.join("\n") + "\n").unwrap();
        }
        let mut src = CodexSource::new(vec![dir.path().to_path_buf()]);
        let mut interner = Interner::default();
        let mut projects = ProjectResolver::new();
        let host: Arc<str> = "here".into();
        let mut ctx = Ctx { interner: &mut interner, projects: &mut projects, host: &host, since: None };
        let mut out = Vec::new();
        src.scan(&mut ctx, &mut out);
        (out, src, dir)
    }

    #[test]
    fn one_record_per_request() {
        let limits = r#"{"primary":{"used_percent":34.0,"window_minutes":300,"resets_in_seconds":600},"secondary":{"used_percent":12.5,"window_minutes":10080,"resets_at":1800000000}}"#;
        let lines = vec![
            META.to_string(),
            TURN.to_string(),
            r#"{"timestamp":"2026-11-01T10:00:02Z","type":"response_item","payload":{"type":"message","content":[{"type":"output_text","text":"token_count"}]}}"#.to_string(),
            tc("2026-11-01T10:00:05Z", (1000, 200, 100, 40), (1000, 200, 100, 40), "null"),
            tc("2026-11-01T10:00:06Z", (1000, 200, 100, 40), (1000, 200, 100, 40), "null"), // the same snapshot again
            tc("2026-11-01T10:01:00Z", (2000, 1000, 200, 50), (3000, 1200, 300, 90), limits),
            // A context-size marker, and a total that does not follow from the request: neither is one.
            tc("2026-11-01T10:01:30Z", (0, 0, 0, 0), (3000, 1200, 300, 90), "null"),
            tc("2026-11-01T10:02:00Z", (500, 0, 50, 0), (9000, 1200, 900, 90), "null"),
        ];
        let (out, src, _dir) = scan(&[("rollout-2026-11-01T10-00-00-sess-1.jsonl", lines)]);
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].tokens,
            Tokens { input: 800, output: 100, cache_read: 200, cache_write: 0, cache_write_1h: 0, reasoning: 40 }
        );
        assert_eq!(
            out[1].tokens,
            Tokens { input: 1000, output: 200, cache_read: 1000, cache_write: 0, cache_write_1h: 0, reasoning: 50 }
        );
        assert_eq!((&*out[0].model, &*out[0].project, &*out[0].session), ("gpt-5-codex", "webapp", "sess-1"));
        assert_eq!(out[0].speed, Speed::Standard);
        assert_ne!(out[0].id, out[1].id);
        let l = src.rate_limits().cloned().unwrap();
        assert_eq!(l.primary.as_ref().unwrap().used_percent, 34.0);
        assert_eq!(l.primary.as_ref().unwrap().resets_at, Some(out[1].ts + 600_000));
        assert_eq!(l.secondary.as_ref().unwrap().resets_at, Some(1_800_000_000_000));
    }

    #[test]
    fn a_fork_does_not_bill_its_parents_history_again() {
        let meta = r#"{"timestamp":"2026-11-01T12:00:00.000Z","type":"session_meta","payload":{"id":"child","cwd":"/nowhere/webapp","forked_from_id":"sess-1"}}"#;
        let parent_meta = r#"{"timestamp":"2026-11-01T12:00:00.010Z","type":"session_meta","payload":{"id":"sess-1","cwd":"/elsewhere"}}"#;
        let lines = vec![
            meta.to_string(),
            parent_meta.to_string(),
            r#"{"timestamp":"2026-11-01T12:00:00.020Z","type":"turn_context","payload":{"model":"gpt-5-codex"}}"#
                .to_string(),
            // The parent's two requests, copied in within milliseconds.
            tc("2026-11-01T12:00:00.030Z", (1000, 200, 100, 40), (1000, 200, 100, 40), "null"),
            tc("2026-11-01T12:00:00.050Z", (2000, 1000, 200, 50), (3000, 1200, 300, 90), "null"),
            // The child's own request, seconds later, continuing from the copied total.
            tc("2026-11-01T12:00:09Z", (4000, 3000, 80, 0), (7000, 4200, 380, 90), "null"),
        ];
        let (out, _, _dir) = scan(&[("rollout-2026-11-01T12-00-00-child.jsonl", lines)]);
        assert_eq!(out.len(), 1, "only the child's own request");
        assert_eq!((out[0].tokens.input, out[0].tokens.cache_read, out[0].tokens.output), (1000, 3000, 80));
        assert_eq!((&*out[0].session, &*out[0].project), ("child", "webapp"), "the first meta names the session");

        // A sub-agent is a fork too.
        let sub = r#"{"timestamp":"2026-11-01T12:00:00.000Z","type":"session_meta","payload":{"id":"sub","source":{"subagent":{"thread_spawn":{"parent_thread_id":"sess-1"}}}}}"#;
        assert!(is_fork(&serde_json::from_str::<Value>(sub).unwrap()["payload"]));
        assert!(!is_fork(&serde_json::from_str::<Value>(META).unwrap()["payload"]));
    }

    #[test]
    fn resumed_and_copied_logs_count_once() {
        let first = vec![
            META.to_string(),
            TURN.to_string(),
            tc("2026-11-01T10:00:05Z", (1000, 200, 100, 40), (1000, 200, 100, 40), "null"),
        ];
        // Resumed in a new file: the total carries on, and only the new request is billed.
        let resumed = vec![
            META.replace("10:00:00", "12:00:00"),
            TURN.replace("10:00:01", "12:00:01"),
            tc("2026-11-01T12:00:05Z", (500, 0, 50, 0), (1500, 200, 150, 40), "null"),
            // A new process starts the total again at its first request.
            tc("2026-11-01T12:10:00Z", (700, 0, 70, 0), (700, 0, 70, 0), "null"),
            tc("2026-11-01T12:10:00Z", (700, 0, 70, 0), (700, 0, 70, 0), "null"),
        ];
        let (out, _, _dir) = scan(&[
            ("rollout-2026-11-01T10-00-00-sess-1.jsonl", first.clone()),
            ("rollout-2026-11-01T10-00-00-sess-1-copy.jsonl", first),
            ("rollout-2026-11-01T12-00-00-sess-1.jsonl", resumed),
        ]);
        let mut ids: Vec<&str> = out.iter().map(|e| &*e.id).collect();
        assert_eq!(ids.len(), 4);
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 3, "the copied log repeats an id, which the collector drops");
        let outputs: Vec<u64> = out.iter().map(|e| e.tokens.output).collect();
        assert_eq!(outputs, [100, 100, 50, 70]);
    }

    #[test]
    fn speed_tier_and_listed_requests() {
        let settings = |ts: &str, tier: &str| {
            format!(
                r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"thread_settings_applied","thread_settings":{{"service_tier":{tier}}}}}}}"#
            )
        };
        let record = |ts: &str, input: u64, output: u64, response: &str| {
            format!(
                r#"{{"timestamp":"{ts}","type":"token_usage_record","payload":{{"usage":{{"input_tokens":{input},"output_tokens":{output}}}{response}}}}}"#
            )
        };
        let lines = vec![
            META.to_string(),
            TURN.to_string(),
            settings("2026-11-01T10:00:02Z", r#""priority""#),
            tc("2026-11-01T10:00:05Z", (1000, 200, 100, 40), (1000, 200, 100, 40), "null"),
            settings("2026-11-01T10:00:06Z", "null"),
            // Two requests listed without an id; the snapshot's total accounts for both.
            record("2026-11-01T10:00:10Z", 300, 30, ""),
            record("2026-11-01T10:00:20Z", 400, 40, ""),
            tc("2026-11-01T10:00:21Z", (400, 0, 40, 0), (1700, 200, 170, 40), "null"),
            // Identified requests are billed as they come; snapshots then only move the total.
            record("2026-11-01T10:01:00Z", 600, 60, r#","response_id":"resp_1""#),
            tc("2026-11-01T10:01:01Z", (600, 0, 60, 0), (2300, 200, 230, 40), "null"),
            record("2026-11-01T10:01:00Z", 600, 60, r#","response_id":"resp_1""#),
        ];
        let (out, _, _dir) = scan(&[("rollout-2026-11-01T10-00-00-sess-1.jsonl", lines)]);
        let seen: Vec<(u64, Speed)> = out.iter().map(|e| (e.tokens.output, e.speed)).collect();
        assert_eq!(
            seen,
            [
                (100, Speed::Fast),
                (30, Speed::Standard),
                (40, Speed::Standard),
                (60, Speed::Standard),
                (60, Speed::Standard)
            ]
        );
        assert_eq!(out[3].id, out[4].id, "a response id is its own identity");
        assert!(out[3].id.ends_with(":r:resp_1"));
        assert_eq!(speed(&Value::from("fast")), Speed::Fast);
        assert_eq!(speed(&Value::from("ultrafast")), Speed::Ultrafast);
        assert_eq!(speed(&Value::from("default")), Speed::Standard);
    }

    #[test]
    fn usage_counters() {
        let read = |j: &str| Usage::read(Some(&serde_json::from_str::<Value>(j).unwrap()));
        let u = read(r#"{"input_tokens":10,"cached_input_tokens":4,"output_tokens":5}"#).unwrap();
        assert_eq!((u.total, u.cached, u.reasoning), (15, 4, 0));
        assert!(
            read(r#"{"input_tokens":10,"cached_input_tokens":40,"output_tokens":5}"#).is_none(),
            "more cached than sent"
        );
        assert!(read(r#"{"input_tokens":10,"output_tokens":5,"total_tokens":3}"#).is_none());
        assert!(read(r#"{"input_tokens":10.5,"output_tokens":5}"#).is_none());
        assert!(read(r#"{"output_tokens":5}"#).is_none());
        let twice = u.plus(&u).unwrap();
        assert_eq!(twice.minus(&u), Some(u));
        assert_eq!(u.minus(&twice), None);
        assert!(twice.is_step_from(&u, &u) && !twice.is_step_from(&u, &twice));
    }
}
