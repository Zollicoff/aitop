//! The billing rules: what in a Codex log is a request to be counted.
//!
//! [`FileState`] follows one log from its first line. Each kind of line
//! updates it, and the two that report usage (`token_count` snapshots and
//! `token_usage_record` entries) add the requests they vouch for.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use super::usage::Usage;
use crate::event::{Event, Speed, Tokens, Tool};
use crate::sources::{Ctx, Tail};

/// A fork copies and restamps its parent's history in a burst; the child's
/// own first request comes seconds later.
const FORK_COPY_MAX_GAP_MS: i64 = 1000;

/// A request listed without a response id, waiting for a snapshot to vouch for it.
#[derive(Clone, Debug)]
struct Pending {
    ts: i64,
    model: Arc<str>,
    speed: Speed,
    usage: Usage,
}

#[derive(Default)]
pub(super) struct FileState {
    pub(super) tail: Tail,
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

impl FileState {
    /// The state for a log that is being read again from the top.
    pub(super) fn restarted(tail: Tail) -> FileState {
        FileState { tail, ..Default::default() }
    }

    /// Whether the log's own `session_meta` has been read.
    pub(super) fn has_meta(&self) -> bool {
        self.saw_meta
    }

    /// The first `session_meta`: which session this is, where it runs, and
    /// whether it opens with a copy of a parent's history.
    pub(super) fn session_meta(&mut self, m: &Value, ts: Option<i64>, ctx: &mut Ctx) {
        self.saw_meta = true;
        if let Some(id) = m["id"].as_str().or(m["session_id"].as_str()) {
            self.session = Some(ctx.interner.get(id));
        }
        if let Some(cwd) = m["cwd"].as_str() {
            self.project = Some(ctx.projects.resolve(cwd, ctx.interner));
        }
        if let (Some(ts), true) = (ts, is_fork(m)) {
            self.fork_copies = true;
            self.fork_anchor = ts;
        }
    }

    /// A `turn_context`: the model (and folder) the following requests use.
    pub(super) fn turn_context(&mut self, t: &Value, ctx: &mut Ctx) {
        self.ledger = false;
        if let Some(m) = t["model"].as_str() {
            self.model = Some(ctx.interner.get(m));
        }
        if let Some(cwd) = t["cwd"].as_str() {
            self.project = Some(ctx.projects.resolve(cwd, ctx.interner));
        }
    }

    /// A `thread_settings_applied` event: the speed tier from here on.
    pub(super) fn settings(&mut self, p: &Value) {
        let settings = &p["thread_settings"];
        if settings.is_object() {
            self.speed = speed(&settings["service_tier"]);
        }
    }

    /// A `token_usage_record` line: one request, as newer logs list them.
    pub(super) fn usage_record(
        &mut self,
        p: &Value,
        ts: Option<i64>,
        fallback_session: &Arc<str>,
        ctx: &mut Ctx,
        out: &mut Vec<Event>,
    ) {
        let (Some(ts), Some(usage), Some(model)) = (ts, Usage::read(p.get("usage")), self.model.clone()) else {
            return;
        };
        if self.is_fork_copy(ts) {
            return;
        }
        let this = Pending { ts, model, speed: self.speed, usage };
        let Some(response_id) = p["response_id"].as_str().filter(|r| !r.is_empty()) else {
            self.ledger = false;
            self.pending.get_or_insert_default().push(this);
            return;
        };
        // An identified request stands on its own, and vouches for the ones before it.
        self.ledger = true;
        for earlier in self.pending.take().unwrap_or_default() {
            self.advance(&earlier.usage);
            self.emit(&earlier, None, fallback_session, ctx, out);
        }
        self.advance(&usage);
        self.date_baseline(ts);
        self.emit(&this, Some(response_id), fallback_session, ctx, out);
    }

    /// A `token_count` snapshot: the request just made and the running total.
    pub(super) fn token_count(
        &mut self,
        p: &Value,
        ts: Option<i64>,
        fallback_session: &Arc<str>,
        ctx: &mut Ctx,
        out: &mut Vec<Event>,
    ) {
        let Some(info) = p.get("info").filter(|i| i.is_object()) else { return };
        let (Some(ts), Some(model)) = (ts, self.model.clone()) else { return };
        let last = Usage::read(info.get("last_token_usage"));
        let has_total = info.get("total_token_usage").is_some_and(|t| !t.is_null());
        let total = if has_total { Usage::read(info.get("total_token_usage")) } else { None };

        if self.is_fork_copy(ts) {
            // Billed in the parent's log; here it only says where the total stands.
            if let Some(t) = total {
                self.rebase(t, ts);
            } else if let (false, Some(l)) = (has_total, last.filter(|l| Some(*l) != self.last_billed)) {
                self.advance(&l);
                self.date_baseline(ts);
            }
            self.last_billed = last;
            return;
        }
        if self.ledger {
            if let Some(t) = total {
                self.rebase(t, ts);
            }
            return;
        }

        let (previous, previous_at, pending) = (self.baseline, self.baseline_at, self.pending.clone());
        // A valid total is the next baseline even if its request turns out not to be billable.
        if let Some(t) = total {
            self.rebase(t, ts);
        }
        let Some(last) = last.filter(|_| !has_total || total.is_some()) else { return };
        if last.is_empty() {
            self.last_billed = None;
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
            if restarted && self.restart == Some((ts, last)) {
                // The same event again.
                self.baseline = previous;
                if previous_at.is_none() {
                    self.baseline_at = None;
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
                        self.baseline = previous;
                        if pending.is_some() {
                            self.pending = pending;
                        }
                    }
                    return;
                };
                unreported = listed;
            }
            if restarted {
                self.restart = Some((ts, last));
            }
        } else {
            if Some(last) == self.last_billed {
                return;
            }
            let Some(advanced) = previous.unwrap_or_default().plus(&last) else { return };
            self.baseline = Some(advanced);
            self.date_baseline(ts);
        }
        self.last_billed = Some(last);

        for earlier in &unreported {
            self.emit(earlier, None, fallback_session, ctx, out);
        }
        self.emit(&Pending { ts, model, speed: self.speed, usage: last }, None, fallback_session, ctx, out);
    }

    /// Add one request to `out`.
    fn emit(
        &mut self,
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
        let session = self.session.clone().unwrap_or_else(|| fallback_session.clone());
        // The same request read from two files (a moved or copied log) gets the
        // same id; equal requests within a file are told apart by their order.
        let id = match response_id {
            Some(id) => format!("x2:{session}:r:{id}"),
            None => {
                let nth = self.repeats.entry((r.ts, u)).or_default();
                *nth += 1;
                format!("x2:{session}:{}:{}.{}.{}:{nth}", r.ts, u.input, u.cached, u.output)
            }
        };
        let (project, project_path) =
            self.project.clone().unwrap_or_else(|| (ctx.interner.get("unknown"), ctx.interner.get("unknown")));
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
}

/// A forked session or a sub-agent: its log opens with its parent's history.
pub(super) fn is_fork(meta: &Value) -> bool {
    meta["forked_from_id"].is_string() || meta["source"]["subagent"]["thread_spawn"]["parent_thread_id"].is_string()
}

/// Missing, default and standard tiers are standard; "fast" is another name for priority.
pub(super) fn speed(service_tier: &Value) -> Speed {
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
