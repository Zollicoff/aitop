//! Time ranges, sort orders and series buckets.

use chrono::{DateTime, Datelike, Duration as CDuration, Local, NaiveDate, TimeZone};
use serde::Serialize;

pub const HOUR: i64 = 3_600_000;

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

    /// The range `delta` steps after this one, wrapping around.
    pub fn cycle(self, delta: isize) -> Range {
        let i = Range::ALL.iter().position(|&r| r == self).unwrap_or(0) as isize;
        Range::ALL[(i + delta).rem_euclid(Range::ALL.len() as isize) as usize]
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

/// Granularity of a cost series.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Bucket {
    Hour,
    Day,
    Week,
}

impl Bucket {
    pub fn label(self) -> &'static str {
        match self {
            Bucket::Hour => "hour",
            Bucket::Day => "day",
            Bucket::Week => "week",
        }
    }
}

/// Bucket boundaries for the cost series of `range`.
pub(super) fn buckets(range: Range, now: DateTime<Local>, first_ts: Option<i64>) -> (Vec<i64>, Bucket) {
    let today = now.date_naive();
    match range {
        Range::Today => {
            // Up to and including the current hour, so the chart ends at "now".
            let m = local_midnight(today);
            let hours = ((now.timestamp_millis() - m) / HOUR + 1).clamp(1, 25);
            ((0..hours).map(|h| m + h * HOUR).collect(), Bucket::Hour)
        }
        Range::Week | Range::Month => {
            let n = range.days().unwrap();
            ((0..n).map(|i| local_midnight(today - CDuration::days(n - 1 - i))).collect(), Bucket::Day)
        }
        Range::All => {
            let first = first_ts
                .and_then(|t| Local.timestamp_millis_opt(t).single())
                .map(|t| t.date_naive())
                .unwrap_or(today)
                .min(today);
            let days = (today - first).num_days() + 1;
            if days <= 120 {
                ((0..days).map(|i| local_midnight(first + CDuration::days(i))).collect(), Bucket::Day)
            } else {
                // Weeks starting on Monday.
                let monday = first - CDuration::days(first.weekday().num_days_from_monday() as i64);
                let weeks = (today - monday).num_days() / 7 + 1;
                ((0..weeks).map(|i| local_midnight(monday + CDuration::weeks(i))).collect(), Bucket::Week)
            }
        }
    }
}
