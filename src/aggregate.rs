//! Turn the event list into what the UI and reports display.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration as CDuration, Local, NaiveDate, TimeZone};

use serde::Serialize;

use crate::event::{Event, Tokens, Tool};

pub const HOUR: i64 = 3_600_000;
/// Claude subscription usage is metered in rolling 5-hour windows.
pub const BLOCK: i64 = 5 * HOUR;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Range {
    Today,
    Week,
    Month,
    All,
}

impl Range {
    pub const ALL: [Range; 4] = [Range::Today, Range::Week, Range::Month, Range::All];

    pub fn short(self) -> &'static str {
        match self {
            Range::Today => "today",
            Range::Week => "week",
            Range::Month => "month",
            Range::All => "all",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Range::Today => "today",
            Range::Week => "last 7 days",
            Range::Month => "last 30 days",
            Range::All => "all time",
        }
    }

    pub fn parse(s: &str) -> Option<Range> {
        match s.to_ascii_lowercase().as_str() {
            "today" | "day" | "d" => Some(Range::Today),
            "week" | "7d" | "w" => Some(Range::Week),
            "month" | "30d" | "m" => Some(Range::Month),
            "all" | "a" => Some(Range::All),
            _ => None,
        }
    }

    fn days(self) -> Option<i64> {
        match self {
            Range::Today => Some(1),
            Range::Week => Some(7),
            Range::Month => Some(30),
            Range::All => None,
        }
    }

    /// Inclusive start of the range (unix ms), or `None` for all time.
    pub fn start(self, now: DateTime<Local>) -> Option<i64> {
        self.days().map(|d| local_midnight(now.date_naive() - CDuration::days(d - 1)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    Cost,
    Tokens,
    Recent,
    Name,
}

impl Sort {
    pub fn next(self) -> Sort {
        match self {
            Sort::Cost => Sort::Tokens,
            Sort::Tokens => Sort::Recent,
            Sort::Recent => Sort::Name,
            Sort::Name => Sort::Cost,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Sort::Cost => "cost",
            Sort::Tokens => "tokens",
            Sort::Recent => "recent",
            Sort::Name => "name",
        }
    }
}

pub fn local_midnight(d: NaiveDate) -> i64 {
    let naive = d.and_hms_opt(0, 0, 0).expect("midnight exists");
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => t.timestamp_millis(),
        // Midnight skipped by a DST change: use 01:00.
        chrono::LocalResult::None => Local
            .from_local_datetime(&(naive + CDuration::hours(1)))
            .earliest()
            .map(|t| t.timestamp_millis())
            .unwrap_or_else(|| naive.and_utc().timestamp_millis()),
    }
}

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

#[derive(Clone, Debug, Default, Serialize)]
pub struct Block {
    pub start: i64,
    pub end: i64,
    pub cost: f64,
    pub tokens: u64,
    pub messages: u64,
    pub burn_per_hour: f64,
    pub projected: f64,
    pub active: bool,
}

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
    /// Most recent activity, regardless of range.
    pub last_ts: Option<i64>,
    pub active_sessions: usize,
    pub today_cost: f64,
    pub today_messages: u64,
    /// Current (or most recent) 5-hour window. Claude only.
    pub block: Option<Block>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub range: Range,
    pub now: i64,
    /// Start of each series bucket (unix ms).
    pub buckets: Vec<i64>,
    /// "hour", "day" or "week".
    pub bucket_unit: &'static str,
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

#[derive(Default)]
struct Acc {
    name: Arc<str>,
    detail: Arc<str>,
    tokens: Tokens,
    cost: f64,
    messages: u64,
    sessions: HashSet<Arc<str>>,
    models: HashMap<Arc<str>, f64>,
    unpriced: bool,
    first_ts: i64,
    last_ts: i64,
}

impl Acc {
    fn add(&mut self, e: &Event) {
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

    fn top_model(&self) -> String {
        self.models
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(m, _)| m.to_string())
            .unwrap_or_default()
    }

    fn row(&self, key: &str, detail: String) -> Row {
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

pub fn sort_rows(rows: &mut [Row], sort: Sort) {
    match sort {
        Sort::Cost => rows.sort_by(|a, b| {
            b.cost
                .partial_cmp(&a.cost)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.tokens.total().cmp(&a.tokens.total()))
        }),
        Sort::Tokens => rows.sort_by_key(|r| std::cmp::Reverse(r.tokens.total())),
        Sort::Recent => rows.sort_by_key(|r| std::cmp::Reverse(r.last_ts)),
        Sort::Name => rows.sort_by_key(|r| r.name.to_lowercase()),
    }
}

/// Bucket boundaries for the cost series of `range`.
fn buckets(range: Range, now: DateTime<Local>, first_ts: Option<i64>) -> (Vec<i64>, &'static str) {
    let today = now.date_naive();
    match range {
        Range::Today => {
            // Up to and including the current hour, so the chart ends at "now".
            let m = local_midnight(today);
            let hours = ((now.timestamp_millis() - m) / HOUR + 1).clamp(1, 25);
            ((0..hours).map(|h| m + h * HOUR).collect(), "hour")
        }
        Range::Week | Range::Month => {
            let n = range.days().unwrap();
            ((0..n).map(|i| local_midnight(today - CDuration::days(n - 1 - i))).collect(), "day")
        }
        Range::All => {
            let first = first_ts
                .and_then(|t| Local.timestamp_millis_opt(t).single())
                .map(|t| t.date_naive())
                .unwrap_or(today)
                .min(today);
            let days = (today - first).num_days() + 1;
            if days <= 120 {
                ((0..days).map(|i| local_midnight(first + CDuration::days(i))).collect(), "day")
            } else {
                // Weeks starting on Monday.
                let monday = first - CDuration::days(first.weekday().num_days_from_monday() as i64);
                let weeks = (today - monday).num_days() / 7 + 1;
                ((0..weeks).map(|i| local_midnight(monday + CDuration::weeks(i))).collect(), "week")
            }
        }
    }
}

pub fn summarize(events: &[Event], range: Range, sort: Sort, now: DateTime<Local>, active_window_ms: i64) -> Summary {
    let now_ms = now.timestamp_millis();
    let start = range.start(now);
    let lo = start.map(|s| events.partition_point(|e| e.ts < s)).unwrap_or(0);
    let in_range = &events[lo..];
    let first_ts = events.first().map(|e| e.ts);
    let (bucket_starts, unit) = buckets(range, now, first_ts);
    let today_start = Range::Today.start(now).unwrap_or(0);
    let today_lo = events.partition_point(|e| e.ts < today_start);
    let active_since = now_ms - active_window_ms;

    let mut tools = Vec::with_capacity(3);
    for tool in Tool::ALL {
        let mut total = Acc::default();
        let mut projects: HashMap<Arc<str>, Acc> = HashMap::new();
        let mut models: HashMap<Arc<str>, Acc> = HashMap::new();
        let mut sessions: HashMap<Arc<str>, Acc> = HashMap::new();
        let mut series = vec![0.0; bucket_starts.len()];
        let mut unpriced: HashSet<Arc<str>> = HashSet::new();
        for e in in_range.iter().filter(|e| e.tool == tool) {
            total.add(e);
            let p = projects.entry(e.project_path.clone()).or_default();
            p.name = e.project.clone();
            p.detail = e.project_path.clone();
            p.add(e);
            let m = models.entry(e.model.clone()).or_default();
            m.name = e.model.clone();
            m.add(e);
            let s = sessions.entry(e.session.clone()).or_default();
            s.name = e.project.clone();
            s.detail = e.session.clone();
            s.add(e);
            if !e.priced {
                unpriced.insert(e.model.clone());
            }
            let idx = bucket_starts.partition_point(|&b| b <= e.ts);
            if idx > 0 {
                series[idx - 1] += e.cost;
            }
        }

        let mut project_rows: Vec<Row> = projects.iter().map(|(k, a)| a.row(k, a.detail.to_string())).collect();
        disambiguate(&mut project_rows);
        let mut model_rows: Vec<Row> = models.iter().map(|(k, a)| a.row(k, String::new())).collect();
        let mut session_rows: Vec<Row> = sessions.iter().map(|(k, a)| a.row(k, a.top_model())).collect();
        for rows in [&mut project_rows, &mut model_rows, &mut session_rows] {
            sort_rows(rows, sort);
        }

        let mut last_ts = None;
        let mut active: HashSet<&Arc<str>> = HashSet::new();
        let mut today_cost = 0.0;
        let mut today_messages = 0;
        for e in events[today_lo..].iter().filter(|e| e.tool == tool) {
            today_cost += e.cost;
            today_messages += 1;
        }
        for e in events.iter().rev().filter(|e| e.tool == tool) {
            last_ts.get_or_insert(e.ts);
            if e.ts < active_since {
                break;
            }
            active.insert(&e.session);
        }
        let mut names: Vec<String> = unpriced.iter().map(|m| m.to_string()).collect();
        names.sort();

        tools.push(ToolSummary {
            tool,
            tokens: total.tokens,
            cost: total.cost,
            messages: total.messages,
            sessions: total.sessions.len(),
            unpriced_models: names,
            projects: project_rows,
            models: model_rows,
            session_rows,
            series,
            last_ts,
            active_sessions: active.len(),
            today_cost,
            today_messages,
            block: if tool == Tool::Claude {
                current_block(events.iter().filter(|e| e.tool == tool), now_ms)
            } else {
                None
            },
        });
    }
    Summary {
        range,
        now: now_ms,
        buckets: bucket_starts,
        bucket_unit: unit,
        total_cost: tools.iter().map(|t| t.cost).sum(),
        total_tokens: tools.iter().map(|t| t.tokens.total()).sum(),
        active_sessions: tools.iter().map(|t| t.active_sessions).sum(),
        tools,
    }
}

/// Two different repositories with the same folder name get their parent
/// directory prepended so they can be told apart.
fn disambiguate(rows: &mut [Row]) {
    let mut count: HashMap<String, usize> = HashMap::new();
    for r in rows.iter() {
        *count.entry(r.name.clone()).or_default() += 1;
    }
    for r in rows.iter_mut() {
        if count[&r.name] > 1 {
            let parent = std::path::Path::new(&r.key)
                .parent()
                .and_then(|p| p.file_name())
                .map(|p| p.to_string_lossy().to_string());
            if let Some(p) = parent {
                r.name = format!("{p}/{}", r.name);
            }
        }
    }
}

/// Group events into 5-hour windows (a window starts at the hour of the first
/// message after the previous one expired) and return the latest.
pub fn current_block<'a>(events: impl Iterator<Item = &'a Event>, now: i64) -> Option<Block> {
    let mut cur: Option<(Block, i64)> = None;
    for e in events {
        match &mut cur {
            Some((b, first)) if e.ts < b.end => {
                b.cost += e.cost;
                b.tokens += e.tokens.total();
                b.messages += 1;
                let _ = first;
            }
            _ => {
                let start = e.ts - e.ts.rem_euclid(HOUR);
                cur = Some((
                    Block {
                        start,
                        end: start + BLOCK,
                        cost: e.cost,
                        tokens: e.tokens.total(),
                        messages: 1,
                        ..Default::default()
                    },
                    e.ts,
                ));
            }
        }
    }
    let (mut b, first) = cur?;
    b.active = now < b.end && now >= b.start;
    let elapsed = (now.min(b.end) - first).max(10 * 60_000) as f64 / HOUR as f64;
    b.burn_per_hour = b.cost / elapsed;
    b.projected = if b.active { b.cost + b.burn_per_hour * (b.end - now) as f64 / HOUR as f64 } else { b.cost };
    Some(b)
}

/// One row per local day, for `aitop report --by day`.
pub fn daily(events: &[Event], tool: Option<Tool>, range: Range, now: DateTime<Local>) -> Vec<Row> {
    let start = range.start(now);
    let lo = start.map(|s| events.partition_point(|e| e.ts < s)).unwrap_or(0);
    let mut days: HashMap<NaiveDate, Acc> = HashMap::new();
    for e in events[lo..].iter().filter(|e| tool.is_none_or(|t| e.tool == t)) {
        let Some(d) = Local.timestamp_millis_opt(e.ts).single().map(|t| t.date_naive()) else { continue };
        days.entry(d).or_default().add(e);
    }
    let mut keys: Vec<_> = days.keys().copied().collect();
    keys.sort();
    keys.into_iter()
        .map(|d| {
            let a = &days[&d];
            let mut r = a.row(&d.to_string(), a.top_model());
            r.name = d.format("%Y-%m-%d %a").to_string();
            r
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Interner;
    const DAY: i64 = 24 * HOUR;

    fn ev(i: &mut Interner, tool: Tool, ts: i64, project: &str, session: &str, cost: f64) -> Event {
        Event {
            tool,
            id: i.get(&format!("{ts}{project}{session}")),
            ts,
            session: i.get(session),
            project: i.get(project.rsplit('/').next().unwrap()),
            project_path: i.get(project),
            model: i.get("m"),
            tokens: Tokens { input: 10, output: 5, ..Default::default() },
            fast: false,
            cost,
            priced: true,
        }
    }

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
        let all = summarize(&events, Range::All, Sort::Cost, now, 5 * 60_000);
        assert_eq!(all.tool(Tool::Claude).cost, 8.5);
        assert_eq!(all.tool(Tool::Codex).cost, 3.0);
        assert_eq!(all.total_cost, 11.5);
        let week = summarize(&events, Range::Week, Sort::Cost, now, 5 * 60_000);
        let c = week.tool(Tool::Claude);
        assert_eq!(c.cost, 3.5);
        assert_eq!(c.sessions, 2);
        assert_eq!(c.active_sessions, 1);
        assert_eq!(week.buckets.len(), 7);
        assert!((c.series.iter().sum::<f64>() - 3.5).abs() < 1e-9);
        // same folder name in two places is disambiguated
        let names: Vec<&str> = c.projects.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["b/web", "a/web", "api"]);
        let month = summarize(&events, Range::Month, Sort::Recent, now, 5 * 60_000);
        assert_eq!(month.tool(Tool::Claude).projects[0].name, "api");
        assert!(all.tool(Tool::Codex).block.is_none());
    }

    #[test]
    fn five_hour_blocks() {
        let mut i = Interner::default();
        let h = |x: f64| (x * HOUR as f64) as i64;
        let base = 1_800_000_000_000 - 1_800_000_000_000 % HOUR;
        let events = [
            ev(&mut i, Tool::Claude, base + h(0.5), "/p", "s", 1.0),
            ev(&mut i, Tool::Claude, base + h(4.9), "/p", "s", 1.0),
            ev(&mut i, Tool::Claude, base + h(5.2), "/p", "s", 2.0), // new block at base+5h
            ev(&mut i, Tool::Claude, base + h(6.0), "/p", "s", 2.0),
        ];
        let b = current_block(events.iter(), base + h(7.2)).unwrap();
        assert_eq!(b.start, base + h(5.0));
        assert_eq!(b.end, base + h(10.0));
        assert_eq!(b.cost, 4.0);
        assert!(b.active);
        assert!((b.burn_per_hour - 4.0 / 2.0).abs() < 1e-9);
        assert!((b.projected - (4.0 + 2.0 * 2.8)).abs() < 1e-6);
        let later = current_block(events.iter(), base + h(11.0)).unwrap();
        assert!(!later.active);
    }
}
