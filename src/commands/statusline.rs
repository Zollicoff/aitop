//! `aitop statusline`: a one-line summary for tmux, a shell prompt or
//! Claude Code's status line.
//!
//! It reads only recently modified logs and skips the history file, so it
//! stays fast enough to run every few seconds. It never reads stdin (Claude
//! Code passes session JSON there, which is not needed).

use std::fmt::Write as _;

use anyhow::{bail, Result};
use chrono::{DateTime, Duration as CDuration, Local};
use ratatui::style::Color;

use super::Context;
use crate::aggregate::{local_midnight, summarize, Range, Sort, Summary, HOUR};
use crate::budget::{self, Budget, Level, Period};
use crate::collector::Collector;
use crate::event::Tool;
use crate::format;
use crate::limits::RateLimits;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorMode {
    None,
    /// ANSI escapes (terminals, Claude Code's status line, shell prompts).
    Ansi,
    /// tmux `#[fg=...]` styles.
    Tmux,
}

#[derive(clap::Args)]
pub struct Args {
    /// Output template, e.g. "{claude} · 5h {block} {block_left}". See `--fields`.
    #[arg(long)]
    pub format: Option<String>,
    #[arg(long, value_enum, default_value = "none")]
    pub color: ColorMode,
    /// List the placeholders `--format` accepts.
    #[arg(long)]
    pub fields: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Today,
    Week,
    Month,
    Claude,
    Codex,
    Block,
    BlockLeft,
    Burn,
    Codex5h,
    CodexWeek,
    Budget,
    Active,
}

const FIELDS: [(&str, Field, &str); 12] = [
    ("today", Field::Today, "cost today, all tools"),
    ("week", Field::Week, "cost over the last 7 days"),
    ("month", Field::Month, "cost over the last 30 days"),
    ("claude", Field::Claude, "Claude Code cost today"),
    ("codex", Field::Codex, "Codex cost today"),
    ("block", Field::Block, "Claude cost in the current 5-hour window"),
    ("block_left", Field::BlockLeft, "time left in Claude's 5-hour window"),
    ("burn", Field::Burn, "Claude spend rate in the 5-hour window, per hour"),
    ("codex_5h", Field::Codex5h, "Codex 5-hour rate limit used"),
    ("codex_week", Field::CodexWeek, "Codex weekly rate limit used"),
    ("budget", Field::Budget, "the budget closest to its limit, e.g. \"day 62%\""),
    ("active", Field::Active, "sessions active in the last few minutes"),
];

#[derive(Debug)]
enum Piece {
    Text(String),
    Field(Field),
}

fn parse(template: &str) -> Result<Vec<Piece>> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        let (before, after) = rest.split_at(i);
        out.push(Piece::Text(before.to_string()));
        if let Some(r) = after.strip_prefix("{{") {
            out.push(Piece::Text("{".into()));
            rest = r;
        } else if let Some(r) = after.strip_prefix("}}") {
            out.push(Piece::Text("}".into()));
            rest = r;
        } else if after.starts_with('}') {
            bail!("unmatched '}}' in format (write '}}}}' for a literal brace)");
        } else {
            let Some(end) = after.find('}') else { bail!("unclosed '{{' in format") };
            let name = &after[1..end];
            let Some((_, f, _)) = FIELDS.iter().find(|(n, _, _)| *n == name) else {
                let names: Vec<&str> = FIELDS.iter().map(|(n, _, _)| *n).collect();
                bail!("unknown field {{{name}}}; available: {}", names.join(", "));
            };
            out.push(Piece::Field(*f));
            rest = &after[end + 1..];
        }
    }
    out.push(Piece::Text(rest.to_string()));
    Ok(out)
}

/// Everything the fields are computed from.
struct Values {
    today: Summary,
    week: Option<Summary>,
    month: Option<Summary>,
    limits: Vec<Option<RateLimits>>,
    budgets: Vec<Budget>,
    detected: Vec<bool>,
    now: i64,
}

/// A piece of output and the colour it would have.
type Span = (String, Option<Color>);

impl Values {
    fn field(&self, f: Field) -> Span {
        let claude = self.today.tool(Tool::Claude);
        let block = claude.block.as_ref().filter(|b| b.active);
        let none = || ("–".to_string(), None);
        let limit = |window: fn(&RateLimits) -> Option<&crate::limits::LimitWindow>| {
            let l = self.limits.get(Tool::Codex.index())?.as_ref()?;
            let w = window(l)?;
            // A window whose reset time has passed has been reset.
            let used = if w.resets_at.is_some_and(|r| r <= self.now) { 0.0 } else { w.used_percent };
            Some((format!("{used:.0}%"), Some(level_color(used / 100.0))))
        };
        match f {
            Field::Today => (format::cost(self.today.total_cost), None),
            Field::Week => self.week.as_ref().map(|s| (format::cost(s.total_cost), None)).unwrap_or_else(none),
            Field::Month => self.month.as_ref().map(|s| (format::cost(s.total_cost), None)).unwrap_or_else(none),
            Field::Claude => (format::cost(claude.cost), Some(theme::current().claude)),
            Field::Codex => (format::cost(self.today.tool(Tool::Codex).cost), Some(theme::current().codex)),
            Field::Block => block.map(|b| (format::cost(b.cost), None)).unwrap_or_else(none),
            Field::BlockLeft => block.map(|b| (format::duration(b.end - self.now), None)).unwrap_or_else(none),
            Field::Burn => block.map(|b| (format!("{}/h", format::cost(b.burn_per_hour)), None)).unwrap_or_else(none),
            Field::Codex5h => limit(|l| l.primary.as_ref()).unwrap_or_else(none),
            Field::CodexWeek => limit(|l| l.secondary.as_ref()).unwrap_or_else(none),
            Field::Budget => self
                .budgets
                .iter()
                .max_by(|a, b| a.fraction().total_cmp(&b.fraction()))
                .map(|b| {
                    let text = format!("{} {:.0}%", b.period.short(), b.fraction() * 100.0);
                    (text, Some(budget_color(b.level)))
                })
                .unwrap_or_else(none),
            Field::Active => (self.today.active_sessions.to_string(), None),
        }
    }

    /// Tools with logs, budget if any: `Claude $4.49 · 5h $0.08 4h 32m | Codex $1.20 · 5h 34% | day 62%`.
    fn default_line(&self) -> Vec<Span> {
        let mut groups: Vec<Vec<Span>> = Vec::new();
        let label = |t: Tool| -> Span {
            let name = match t {
                Tool::Claude => "Claude",
                Tool::Codex => "Codex",
            };
            (
                name.to_string(),
                Some(match t {
                    Tool::Claude => theme::current().claude,
                    Tool::Codex => theme::current().codex,
                }),
            )
        };
        if self.detected[Tool::Claude.index()] {
            let mut g = vec![label(Tool::Claude), (" ".into(), None), self.field(Field::Claude)];
            if self.today.tool(Tool::Claude).block.as_ref().is_some_and(|b| b.active) {
                g.extend([
                    (" · 5h ".into(), None),
                    self.field(Field::Block),
                    (" ".into(), None),
                    self.field(Field::BlockLeft),
                ]);
            }
            groups.push(g);
        }
        if self.detected[Tool::Codex.index()] {
            let mut g = vec![label(Tool::Codex), (" ".into(), None), self.field(Field::Codex)];
            if self.limits.get(Tool::Codex.index()).is_some_and(|l| l.is_some()) {
                g.extend([(" · 5h ".into(), None), self.field(Field::Codex5h)]);
            }
            groups.push(g);
        }
        if !self.budgets.is_empty() {
            groups.push(vec![self.field(Field::Budget)]);
        }
        if groups.is_empty() {
            return vec![("aitop: no logs".into(), None)];
        }
        let mut out = Vec::new();
        for (i, g) in groups.into_iter().enumerate() {
            if i > 0 {
                out.push((" | ".into(), None));
            }
            out.extend(g);
        }
        out
    }
}

fn level_color(frac: f64) -> Color {
    let p = theme::current();
    if frac >= 0.95 {
        p.bad
    } else if frac >= 0.8 {
        p.warn
    } else {
        p.good
    }
}

fn budget_color(level: Level) -> Color {
    let p = theme::current();
    match level {
        Level::Ok => p.good,
        Level::Warn => p.warn,
        Level::Over => p.bad,
    }
}

fn render(spans: &[Span], mode: ColorMode) -> String {
    let mut out = String::new();
    for (text, color) in spans {
        match (mode, color) {
            (ColorMode::Ansi, Some(c)) => {
                let _ = write!(out, "{}{text}\x1b[39m", theme::ansi(theme::for_output(*c), false));
            }
            (ColorMode::Tmux, Some(c)) => {
                let _ = write!(out, "{}{text}#[fg=default]", theme::tmux(*c));
            }
            _ => out.push_str(text),
        }
    }
    out
}

/// Earliest time the requested fields need logs from.
fn lookback(fields: &[Field], budgets: &[Budget], now: DateTime<Local>) -> i64 {
    // Claude's 5-hour windows chain from one to the next, so read a day
    // back to be sure to include a gap that resets the chain.
    let mut since = local_midnight(now.date_naive()).min(now.timestamp_millis() - 24 * HOUR);
    let days_back = |d: i64| local_midnight(now.date_naive() - CDuration::days(d - 1));
    for f in fields {
        since = since.min(match f {
            Field::Week => days_back(7),
            Field::Month => days_back(30),
            _ => since,
        });
    }
    for b in budgets {
        let start = match b.period {
            Period::Day => local_midnight(now.date_naive()),
            Period::Week => days_back(8),
            Period::Month => days_back(32),
        };
        since = since.min(start);
    }
    since
}

pub fn run(ctx: &Context, args: &Args) -> Result<()> {
    if args.fields {
        for (name, _, help) in FIELDS {
            println!("{{{name}}}{}{help}", " ".repeat(14usize.saturating_sub(name.len() + 2)));
        }
        return Ok(());
    }
    let pieces = args.format.as_deref().map(parse).transpose()?;
    let fields: Vec<Field> = match &pieces {
        Some(p) => p.iter().filter_map(|x| if let Piece::Field(f) = x { Some(*f) } else { None }).collect(),
        None => vec![Field::Claude, Field::Codex, Field::Block, Field::Codex5h, Field::Budget],
    };
    let now = Local::now();
    // Budgets need their whole period; evaluate them with empty data first
    // just to learn which periods are configured.
    let wants_budget = fields.contains(&Field::Budget);
    let configured = if wants_budget { budget::evaluate(&[], &ctx.cfg.budget, now) } else { Vec::new() };
    let mut c = Collector::new(&ctx.cfg, None);
    c.read_since(lookback(&fields, &configured, now));
    c.scan();
    let d = c.data.read().unwrap();
    let summary = |r: Range| summarize(&d.events, r, Sort::Cost, now, ctx.active_window_ms, &[]);
    let values = Values {
        today: summary(Range::Today),
        week: fields.contains(&Field::Week).then(|| summary(Range::Week)),
        month: fields.contains(&Field::Month).then(|| summary(Range::Month)),
        limits: d.limits.clone(),
        budgets: if wants_budget { budget::evaluate(&d.events, &ctx.cfg.budget, now) } else { Vec::new() },
        detected: d.status.iter().map(|s| s.detected()).collect(),
        now: now.timestamp_millis(),
    };
    let spans: Vec<Span> = match &pieces {
        Some(p) => p
            .iter()
            .map(|x| match x {
                Piece::Text(t) => (t.clone(), None),
                Piece::Field(f) => values.field(*f),
            })
            .collect(),
        None => values.default_line(),
    };
    println!("{}", render(&spans, args.color));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates() {
        let p = parse("{claude} {{x}} · {block_left}").unwrap();
        let fields: Vec<Field> =
            p.iter().filter_map(|x| if let Piece::Field(f) = x { Some(*f) } else { None }).collect();
        assert_eq!(fields, [Field::Claude, Field::BlockLeft]);
        let text: String =
            p.iter().filter_map(|x| if let Piece::Text(t) = x { Some(t.as_str()) } else { None }).collect();
        assert_eq!(text, " {x} · ");
        assert!(parse("{nope}").unwrap_err().to_string().contains("available: today"));
        assert!(parse("{claude").is_err());
        assert!(parse("a } b").is_err());
    }

    #[test]
    fn rendering() {
        let spans = vec![("Claude".to_string(), Some(Color::Rgb(1, 2, 3))), (" $1.00".to_string(), None)];
        assert_eq!(render(&spans, ColorMode::None), "Claude $1.00");
        assert_eq!(render(&spans, ColorMode::Tmux), "#[fg=#010203]Claude#[fg=default] $1.00");
        assert!(render(&spans, ColorMode::Ansi).ends_with("Claude\x1b[39m $1.00"));
    }

    #[test]
    fn lookback_covers_what_fields_need() {
        let now = Local::now();
        let day = lookback(&[Field::Today], &[], now);
        assert!(day <= now.timestamp_millis() - 24 * HOUR);
        let month = lookback(&[Field::Month], &[], now);
        assert!(month <= now.timestamp_millis() - 29 * 24 * HOUR);
    }
}
