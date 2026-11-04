//! Spend budgets.
//!
//! ```toml
//! [budget]
//! daily = 20.0          # across all tools, USD
//! monthly = 300.0
//! warn_at = 80          # percent of a budget that counts as "nearly used"
//! week_starts = "monday"
//!
//! [budget.claude]
//! weekly = 100.0
//! ```
//!
//! Budgets use calendar periods in local time: today, this week and this
//! month.

use chrono::{DateTime, Datelike, Duration as CDuration, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::aggregate::local_midnight;
use crate::event::{Event, Tool};

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub daily: Option<f64>,
    pub weekly: Option<f64>,
    pub monthly: Option<f64>,
}

impl Limits {
    fn get(&self, p: Period) -> Option<f64> {
        match p {
            Period::Day => self.daily,
            Period::Week => self.weekly,
            Period::Month => self.monthly,
        }
        .filter(|v| *v > 0.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WeekStart {
    #[default]
    Monday,
    Sunday,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BudgetConfig {
    pub daily: Option<f64>,
    pub weekly: Option<f64>,
    pub monthly: Option<f64>,
    /// Percent of a budget at which it is flagged as nearly used.
    pub warn_at: f64,
    pub week_starts: WeekStart,
    /// Ring the terminal bell when a budget crosses into warning or over.
    pub bell: bool,
    pub claude: Limits,
    pub codex: Limits,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        BudgetConfig {
            daily: None,
            weekly: None,
            monthly: None,
            warn_at: 80.0,
            week_starts: WeekStart::Monday,
            bell: false,
            claude: Limits::default(),
            codex: Limits::default(),
        }
    }
}

impl BudgetConfig {
    fn total(&self) -> Limits {
        Limits { daily: self.daily, weekly: self.weekly, monthly: self.monthly }
    }

    fn for_tool(&self, t: Tool) -> Limits {
        match t {
            Tool::Claude => self.claude,
            Tool::Codex => self.codex,
        }
    }

    pub fn is_empty(&self) -> bool {
        Period::ALL
            .iter()
            .all(|&p| self.total().get(p).is_none() && Tool::ALL.iter().all(|&t| self.for_tool(t).get(p).is_none()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Period {
    Day,
    Week,
    Month,
}

impl Period {
    pub const ALL: [Period; 3] = [Period::Day, Period::Week, Period::Month];

    pub fn label(self) -> &'static str {
        match self {
            Period::Day => "today",
            Period::Week => "this week",
            Period::Month => "this month",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Period::Day => "day",
            Period::Week => "week",
            Period::Month => "month",
        }
    }

    /// First day of the period containing `today`, and of the next one.
    fn days(self, today: NaiveDate, week: WeekStart) -> (NaiveDate, NaiveDate) {
        match self {
            Period::Day => (today, today + CDuration::days(1)),
            Period::Week => {
                let back = match week {
                    WeekStart::Monday => today.weekday().num_days_from_monday(),
                    WeekStart::Sunday => today.weekday().num_days_from_sunday(),
                };
                let start = today - CDuration::days(back as i64);
                (start, start + CDuration::days(7))
            }
            Period::Month => {
                let start = today.with_day(1).expect("day 1 exists");
                let next = if start.month() == 12 {
                    NaiveDate::from_ymd_opt(start.year() + 1, 1, 1)
                } else {
                    NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1)
                };
                (start, next.expect("valid date"))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Warn,
    Over,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Budget {
    /// `None` for a budget across all tools.
    pub tool: Option<Tool>,
    pub period: Period,
    pub limit: f64,
    pub spent: f64,
    /// Spend at the end of the period if the current pace continues.
    pub projected: f64,
    /// Start of the next period (unix ms).
    pub resets_at: i64,
    pub level: Level,
}

impl Budget {
    pub fn fraction(&self) -> f64 {
        self.spent / self.limit
    }

    /// e.g. "Claude Code daily budget" / "daily budget".
    pub fn name(&self) -> String {
        let period = match self.period {
            Period::Day => "daily",
            Period::Week => "weekly",
            Period::Month => "monthly",
        };
        match self.tool {
            Some(t) => format!("{} {period} budget", t.name()),
            None => format!("{period} budget"),
        }
    }
}

/// Every configured budget with what has been spent against it.
pub fn evaluate(events: &[Event], cfg: &BudgetConfig, now: DateTime<Local>) -> Vec<Budget> {
    let now_ms = now.timestamp_millis();
    let mut out = Vec::new();
    for period in Period::ALL {
        let (first, next) = period.days(now.date_naive(), cfg.week_starts);
        let (start, end) = (local_midnight(first), local_midnight(next));
        let lo = events.partition_point(|e| e.ts < start);
        let in_period = &events[lo..];
        let scopes = std::iter::once((None, cfg.total())).chain(Tool::ALL.iter().map(|&t| (Some(t), cfg.for_tool(t))));
        for (tool, limits) in scopes {
            let Some(limit) = limits.get(period) else { continue };
            // `+ 0.0` turns the empty sum (-0.0) into 0.0, which prints without a sign.
            let spent: f64 =
                in_period.iter().filter(|e| tool.is_none_or(|t| e.tool == t)).map(|e| e.cost).sum::<f64>() + 0.0;
            // Project from at least an hour of elapsed time, so the first
            // request of the day doesn't extrapolate wildly.
            let elapsed = (now_ms - start).max(3_600_000) as f64;
            let projected = spent * ((end - start) as f64 / elapsed).max(1.0);
            let level = if spent >= limit {
                Level::Over
            } else if spent >= limit * cfg.warn_at / 100.0 {
                Level::Warn
            } else {
                Level::Ok
            };
            out.push(Budget { tool, period, limit, spent, projected, resets_at: end, level });
        }
    }
    out
}

/// The most used budget in `budgets` that applies to `tool` (or to all tools).
pub fn tightest(budgets: &[Budget], tool: Option<Tool>) -> Option<&Budget> {
    budgets.iter().filter(|b| b.tool == tool).max_by(|a, b| a.fraction().total_cmp(&b.fraction()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::testutil::ev;
    use crate::event::Interner;
    use chrono::TimeZone;

    fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, 0, 0).single().unwrap()
    }

    #[test]
    fn calendar_periods() {
        let wed = NaiveDate::from_ymd_opt(2026, 11, 4).unwrap();
        let d = |y, m, dd| NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        assert_eq!(Period::Day.days(wed, WeekStart::Monday), (wed, d(2026, 11, 5)));
        assert_eq!(Period::Week.days(wed, WeekStart::Monday), (d(2026, 11, 2), d(2026, 11, 9)));
        assert_eq!(Period::Week.days(wed, WeekStart::Sunday), (d(2026, 11, 1), d(2026, 11, 8)));
        assert_eq!(Period::Month.days(wed, WeekStart::Monday), (d(2026, 11, 1), d(2026, 12, 1)));
        assert_eq!(Period::Month.days(d(2026, 12, 31), WeekStart::Monday), (d(2026, 12, 1), d(2027, 1, 1)));
    }

    #[test]
    fn evaluates_levels_and_scopes() {
        let now = at(2026, 11, 4, 12);
        let n = now.timestamp_millis();
        let mut i = Interner::default();
        let events = vec![
            ev(&mut i, Tool::Claude, at(2026, 10, 30, 12).timestamp_millis(), "/p", "a", 50.0), // last month
            ev(&mut i, Tool::Claude, at(2026, 11, 3, 12).timestamp_millis(), "/p", "b", 10.0),  // this week
            ev(&mut i, Tool::Claude, n - 3_600_000, "/p", "c", 8.5),
            ev(&mut i, Tool::Codex, n - 60_000, "/p", "d", 1.0),
        ];
        let cfg: BudgetConfig =
            toml::from_str("daily = 10.0\nmonthly = 100.0\n[claude]\ndaily = 8.0\nweekly = 40.0\n[codex]\ndaily = 0\n")
                .unwrap();
        let b = evaluate(&events, &cfg, now);
        let find = |tool, p| b.iter().find(|x| x.tool == tool && x.period == p).unwrap();
        let total_day = find(None, Period::Day);
        assert_eq!(total_day.spent, 9.5);
        assert_eq!(total_day.level, Level::Warn, "9.5 of 10 is past 80%");
        assert!((total_day.projected - 19.0).abs() < 1e-9, "half the day gone");
        assert_eq!(total_day.resets_at, at(2026, 11, 5, 0).timestamp_millis());
        assert_eq!(find(Some(Tool::Claude), Period::Day).level, Level::Over);
        assert_eq!(find(Some(Tool::Claude), Period::Week).spent, 18.5);
        assert_eq!(find(None, Period::Month).spent, 19.5);
        assert_eq!(find(None, Period::Month).level, Level::Ok);
        assert!(b.iter().all(|x| x.tool != Some(Tool::Codex)), "a zero budget is no budget");
        let empty: BudgetConfig = toml::from_str("[codex]\nweekly = 5.0").unwrap();
        let none = evaluate(&[], &empty, now);
        assert!(none[0].spent.is_sign_positive(), "no spend is +0, not -0");
        assert_eq!(b.len(), 4);
        assert_eq!(tightest(&b, Some(Tool::Claude)).unwrap().period, Period::Day);
        assert!(!cfg.is_empty() && BudgetConfig::default().is_empty());
    }
}
