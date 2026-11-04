//! Per-tool summaries for a range.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Local, NaiveDate, TimeZone};
use serde::Serialize;

use super::block::{current_block, Block};
use super::filter::{matches_all, Filter};
use super::range::{buckets, Bucket, Range, Sort};
use super::rows::{disambiguate, Acc, Groups, Row};
use crate::event::{Event, Tokens, Tool};

#[derive(Clone, Debug, Serialize)]
pub struct ToolSummary {
    pub tool: Tool,
    pub tokens: Tokens,
    pub cost: f64,
    pub messages: u64,
    pub sessions: usize,
    pub unpriced_models: Vec<String>,
    pub projects: Vec<Row>,
    pub models: Vec<Row>,
    pub session_rows: Vec<Row>,
    /// Cost per bucket (see `Summary::buckets`).
    pub series: Vec<f64>,
    /// Tokens per bucket.
    pub series_tokens: Vec<u64>,
    /// Most recent activity, regardless of range.
    pub last_ts: Option<i64>,
    pub active_sessions: usize,
    pub today_cost: f64,
    pub today_messages: u64,
    /// Current (or most recent) 5-hour window, for tools metered that way.
    pub block: Option<Block>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub range: Range,
    pub now: i64,
    /// Start of each series bucket (unix ms).
    pub buckets: Vec<i64>,
    pub bucket: Bucket,
    /// Indexed by `Tool::index()`.
    pub tools: Vec<ToolSummary>,
    pub total_cost: f64,
    pub total_tokens: u64,
    pub active_sessions: usize,
}

impl Summary {
    pub fn tool(&self, t: Tool) -> &ToolSummary {
        &self.tools[t.index()]
    }
}

/// The slices of the (time-sorted) event list a summary needs.
struct Input<'a> {
    all: &'a [Event],
    in_range: &'a [Event],
    today: &'a [Event],
    buckets: &'a [i64],
    sort: Sort,
    now: i64,
    active_since: i64,
    /// Per tool (by index); missing means unfiltered.
    filters: &'a [Vec<Filter>],
}

impl Input<'_> {
    fn filters(&self, tool: Tool) -> &[Filter] {
        self.filters.get(tool.index()).map(Vec::as_slice).unwrap_or(&[])
    }

    /// `tool`'s events in `events` that pass its filters.
    fn selected<'e>(
        &self,
        events: &'e [Event],
        tool: Tool,
    ) -> impl DoubleEndedIterator<Item = &'e Event> + use<'e, '_> {
        let filters = self.filters(tool);
        of_tool(events, tool).filter(move |e| matches_all(filters, e))
    }
}

fn of_tool(events: &[Event], tool: Tool) -> impl DoubleEndedIterator<Item = &Event> {
    events.iter().filter(move |e| e.tool == tool)
}

/// Events at or after `start`, found by binary search.
fn since(events: &[Event], start: Option<i64>) -> &[Event] {
    let lo = start.map(|s| events.partition_point(|e| e.ts < s)).unwrap_or(0);
    &events[lo..]
}

/// Summaries for every tool over `range`. `filters` (indexed by tool) narrow
/// a tool to a project, model and/or session; the 5-hour window is account
/// wide and ignores them.
pub fn summarize(
    events: &[Event],
    range: Range,
    sort: Sort,
    now: DateTime<Local>,
    active_window_ms: i64,
    filters: &[Vec<Filter>],
) -> Summary {
    let now_ms = now.timestamp_millis();
    let (bucket_starts, bucket) = buckets(range, now, events.first().map(|e| e.ts));
    let input = Input {
        all: events,
        in_range: since(events, range.start(now)),
        today: since(events, Range::Today.start(now)),
        buckets: &bucket_starts,
        sort,
        now: now_ms,
        active_since: now_ms - active_window_ms,
        filters,
    };
    let tools: Vec<ToolSummary> = Tool::ALL.into_iter().map(|t| summarize_tool(t, &input)).collect();
    // Totals cover everything, whatever the panels are narrowed to.
    let unfiltered = Input { filters: &[], ..input };
    Summary {
        range,
        now: now_ms,
        bucket,
        total_cost: input.in_range.iter().map(|e| e.cost).sum::<f64>() + 0.0,
        total_tokens: input.in_range.iter().map(|e| e.tokens.total()).sum(),
        active_sessions: Tool::ALL.into_iter().map(|t| activity(t, &unfiltered).1).sum(),
        buckets: bucket_starts,
        tools,
    }
}

fn summarize_tool(tool: Tool, input: &Input) -> ToolSummary {
    let mut total = Acc::default();
    let (mut projects, mut models, mut sessions) = (Groups::default(), Groups::default(), Groups::default());
    let mut series = vec![0.0; input.buckets.len()];
    let mut series_tokens = vec![0u64; input.buckets.len()];
    let mut unpriced = BTreeSet::new();
    for e in input.selected(input.in_range, tool) {
        total.add(e);
        projects.add(&e.project_path, &e.project, &e.project_path, e);
        models.add(&e.model, &e.model, &e.model, e);
        sessions.add(&e.session, &e.project, &e.session, e);
        if !e.priced {
            unpriced.insert(e.model.to_string());
        }
        // Bucket = last start at or before the event.
        if let Some(i) = input.buckets.partition_point(|&b| b <= e.ts).checked_sub(1) {
            series[i] += e.cost;
            series_tokens[i] += e.tokens.total();
        }
    }
    let mut project_rows = projects.into_rows(input.sort, |a| a.detail());
    disambiguate(&mut project_rows);

    let (today_cost, today_messages) =
        input.selected(input.today, tool).fold((0.0, 0), |(c, n), e| (c + e.cost, n + 1));
    let (last_ts, active_sessions) = activity(tool, input);
    ToolSummary {
        tool,
        tokens: total.tokens(),
        cost: total.cost(),
        messages: total.messages(),
        sessions: total.session_count(),
        unpriced_models: unpriced.into_iter().collect(),
        projects: project_rows,
        models: models.into_rows(input.sort, |_| String::new()),
        session_rows: sessions.into_rows(input.sort, Acc::top_model),
        series,
        series_tokens,
        last_ts,
        active_sessions,
        today_cost,
        today_messages,
        block: tool.has_usage_windows().then(|| current_block(of_tool(input.all, tool), input.now)).flatten(),
    }
}

/// Most recent activity and the number of sessions active within the window.
fn activity(tool: Tool, input: &Input) -> (Option<i64>, usize) {
    let mut recent = input.selected(input.all, tool).rev().peekable();
    let last_ts = recent.peek().map(|e| e.ts);
    let active: HashSet<_> = recent.take_while(|e| e.ts >= input.active_since).map(|e| &e.session).collect();
    (last_ts, active.len())
}

/// One row per local day, for `aitop report --by day`.
pub fn daily(events: &[Event], tool: Option<Tool>, range: Range, now: DateTime<Local>) -> Vec<Row> {
    let mut days: HashMap<NaiveDate, Acc> = HashMap::new();
    for e in since(events, range.start(now)).iter().filter(|e| tool.is_none_or(|t| e.tool == t)) {
        if let Some(d) = Local.timestamp_millis_opt(e.ts).single().map(|t| t.date_naive()) {
            days.entry(d).or_default().add(e);
        }
    }
    let mut days: Vec<_> = days.into_iter().collect();
    days.sort_by_key(|(d, _)| *d);
    days.into_iter()
        .map(|(d, a)| {
            let model = a.top_model();
            let mut r = a.into_row(&d.to_string(), model);
            r.name = d.format("%Y-%m-%d %a").to_string();
            r
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::testutil::{ev, DAY};
    use crate::event::Interner;

    #[test]
    fn ranges_and_rows() {
        let now = Local::now();
        let n = now.timestamp_millis();
        let mut i = Interner::default();
        let mut events = vec![
            ev(&mut i, Tool::Claude, n - 40 * DAY, "/a/web", "s0", 5.0),
            ev(&mut i, Tool::Claude, n - 3 * DAY, "/a/web", "s1", 1.0),
            ev(&mut i, Tool::Claude, n - 60_000, "/b/web", "s2", 2.0),
            ev(&mut i, Tool::Claude, n - 30_000, "/a/api", "s2", 0.5),
            ev(&mut i, Tool::Codex, n - 10_000, "/a/api", "x1", 3.0),
        ];
        events.sort_by_key(|e| e.ts);
        let all = summarize(&events, Range::All, Sort::Cost, now, 5 * 60_000, &[]);
        assert_eq!(all.tool(Tool::Claude).cost, 8.5);
        assert_eq!(all.tool(Tool::Codex).cost, 3.0);
        assert_eq!(all.total_cost, 11.5);
        let week = summarize(&events, Range::Week, Sort::Cost, now, 5 * 60_000, &[]);
        let c = week.tool(Tool::Claude);
        assert_eq!(c.cost, 3.5);
        assert_eq!(c.sessions, 2);
        assert_eq!(c.active_sessions, 1);
        assert_eq!(c.last_ts, Some(n - 30_000));
        assert_eq!(week.buckets.len(), 7);
        assert!((c.series.iter().sum::<f64>() - 3.5).abs() < 1e-9);
        assert_eq!(c.series_tokens.iter().sum::<u64>(), c.tokens.total());
        // same folder name in two places is disambiguated
        let names: Vec<&str> = c.projects.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["b/web", "a/web", "api"]);
        assert_eq!(c.session_rows[0].detail, "m", "session rows show their top model");
        let month = summarize(&events, Range::Month, Sort::Recent, now, 5 * 60_000, &[]);
        assert_eq!(month.tool(Tool::Claude).projects[0].name, "api");
        assert!(all.tool(Tool::Codex).block.is_none());
        assert!(all.tool(Tool::Claude).block.is_some());
    }

    #[test]
    fn filters_narrow_one_tool() {
        let now = Local::now();
        let n = now.timestamp_millis();
        let mut i = Interner::default();
        let mut events = vec![
            ev(&mut i, Tool::Claude, n - 50_000, "/a/web", "s1", 1.0),
            ev(&mut i, Tool::Claude, n - 40_000, "/a/api", "s2", 2.0),
            ev(&mut i, Tool::Claude, n - 30_000, "/a/api", "s3", 4.0),
            ev(&mut i, Tool::Codex, n - 20_000, "/a/web", "x1", 8.0),
        ];
        events.sort_by_key(|e| e.ts);
        let api = Filter { kind: crate::aggregate::FilterKind::Project, key: "/a/api".into(), label: "api".into() };
        let filters = vec![vec![api.clone()], vec![]];
        let s = summarize(&events, Range::All, Sort::Cost, now, 5 * 60_000, &filters);
        let c = s.tool(Tool::Claude);
        assert_eq!(c.cost, 6.0);
        assert_eq!(c.sessions, 2);
        assert_eq!(c.projects.len(), 1);
        assert_eq!(c.active_sessions, 2);
        assert_eq!(c.block.as_ref().unwrap().cost, 7.0, "the 5h window ignores filters");
        assert_eq!(s.tool(Tool::Codex).cost, 8.0, "other tools are unaffected");
        assert_eq!(s.total_cost, 15.0, "totals ignore filters");
        assert_eq!(s.active_sessions, 4);
        let s3 = Filter { kind: crate::aggregate::FilterKind::Session, key: "s3".into(), label: "s3".into() };
        let narrower = summarize(&events, Range::All, Sort::Cost, now, 5 * 60_000, &[vec![api, s3]]);
        assert_eq!(narrower.tool(Tool::Claude).cost, 4.0);
    }

    #[test]
    fn daily_rows() {
        let now = Local::now();
        let n = now.timestamp_millis();
        let mut i = Interner::default();
        let events =
            [ev(&mut i, Tool::Claude, n - 2 * DAY, "/p", "a", 1.0), ev(&mut i, Tool::Claude, n - 1000, "/p", "b", 2.0)];
        let rows = daily(&events, None, Range::All, now);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].cost, 2.0);
    }
}
