//! `aitop report`: usage as a plain-text table or JSON.

use std::fmt::Write as _;

use anyhow::Result;
use chrono::Local;
use serde::Serialize;

use super::Context;
use crate::aggregate::{daily, summarize, Block, Range, Row, Sort, Summary};
use crate::event::{Event, Tokens, Tool};
use crate::format;

#[derive(clap::Args)]
pub struct Args {
    /// Group rows by project, model, session or day.
    #[arg(long, value_enum, default_value = "project")]
    pub by: By,
    /// Only one tool.
    #[arg(long, value_enum)]
    pub tool: Option<Tool>,
    #[arg(long, value_enum, default_value = "cost")]
    pub sort: Sort,
    /// Maximum rows per tool.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    /// Machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum By {
    Project,
    Model,
    Session,
    Day,
}

impl By {
    fn key(self) -> &'static str {
        match self {
            By::Project => "project",
            By::Model => "model",
            By::Session => "session",
            By::Day => "day",
        }
    }
}

pub fn run(ctx: &Context, args: &Args) -> Result<()> {
    let c = ctx.snapshot();
    let d = c.data.read().unwrap();
    let summary = summarize(&d.events, ctx.range, args.sort, Local::now(), ctx.active_window_ms);
    let report = Report::new(&summary, &d.events, args);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report.json())?);
    } else {
        print!("{}", report.text(args.limit.max(1)));
    }
    Ok(())
}

/// The rows to print for each selected tool.
struct Report<'a> {
    summary: &'a Summary,
    by: By,
    sections: Vec<(Tool, Vec<Row>)>,
}

impl<'a> Report<'a> {
    fn new(summary: &'a Summary, events: &[Event], args: &Args) -> Self {
        let sections = Tool::ALL
            .into_iter()
            .filter(|t| args.tool.is_none_or(|o| o == *t))
            .map(|t| {
                let ts = summary.tool(t);
                let rows = match args.by {
                    By::Project => ts.projects.clone(),
                    By::Model => ts.models.clone(),
                    By::Session => ts.session_rows.clone(),
                    By::Day => daily(events, Some(t), summary.range, Local::now()),
                };
                (t, rows)
            })
            .collect();
        Report { summary, by: args.by, sections }
    }

    fn total_cost(&self) -> f64 {
        self.sections.iter().map(|(t, _)| self.summary.tool(*t).cost).sum()
    }

    fn total_tokens(&self) -> u64 {
        self.sections.iter().map(|(t, _)| self.summary.tool(*t).tokens.total()).sum()
    }

    fn json(&self) -> JsonReport<'_> {
        JsonReport {
            range: self.summary.range,
            generated_at: Local::now().to_rfc3339(),
            by: self.by.key(),
            total_cost: self.total_cost(),
            total_tokens: self.total_tokens(),
            tools: self
                .sections
                .iter()
                .map(|(t, rows)| {
                    let ts = self.summary.tool(*t);
                    JsonTool {
                        tool: *t,
                        name: t.name(),
                        cost: ts.cost,
                        tokens: ts.tokens,
                        total_tokens: ts.tokens.total(),
                        sessions: ts.sessions,
                        responses: ts.messages,
                        unpriced_models: &ts.unpriced_models,
                        rows,
                        five_hour_window: ts.block.as_ref().filter(|b| b.active),
                    }
                })
                .collect(),
        }
    }

    fn text(&self, limit: usize) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "aitop · {} · {} · {} tokens",
            self.summary.range.label(),
            format::cost(self.total_cost()),
            format::tokens(self.total_tokens())
        );
        for (t, rows) in &self.sections {
            let ts = self.summary.tool(*t);
            let _ = writeln!(
                out,
                "\n{}  {} · {} tokens · {} sessions · {} responses",
                t.name(),
                format::cost(ts.cost),
                format::tokens(ts.tokens.total()),
                format::thousands(ts.sessions as u64),
                format::thousands(ts.messages)
            );
            if rows.is_empty() {
                out += "  (no usage)\n";
                continue;
            }
            let header = self.by.key().to_uppercase();
            let _ = writeln!(
                out,
                "  {header:<34} {:>8} {:>9} {:>9} {:>9} {:>10}",
                "SESSIONS", "INPUT", "OUTPUT", "CACHE", "COST"
            );
            for r in rows.iter().take(limit) {
                self.text_row(&mut out, r);
            }
            if rows.len() > limit {
                let _ = writeln!(out, "  … {} more (use --limit)", rows.len() - limit);
            }
            if !ts.unpriced_models.is_empty() {
                let _ = writeln!(out, "  * no price for: {}", ts.unpriced_models.join(", "));
            }
        }
        out
    }

    fn text_row(&self, out: &mut String, r: &Row) {
        let name = match self.by {
            By::Session => format!("{} {}", r.name, r.key.chars().take(8).collect::<String>()),
            By::Model => format::short_model(&r.name),
            _ => r.name.clone(),
        };
        let cost = if r.unpriced { format!("{}*", format::cost(r.cost)) } else { format::cost(r.cost) };
        let _ = writeln!(
            out,
            "  {:<34} {:>8} {:>9} {:>9} {:>9} {:>10}",
            format::truncate(&name, 34),
            r.sessions,
            format::tokens(r.tokens.input),
            format::tokens(r.tokens.output),
            format::tokens(r.tokens.cache_read + r.tokens.cache_writes()),
            cost
        );
    }
}

#[derive(Serialize)]
struct JsonReport<'a> {
    range: Range,
    generated_at: String,
    by: &'static str,
    total_cost: f64,
    total_tokens: u64,
    tools: Vec<JsonTool<'a>>,
}

#[derive(Serialize)]
struct JsonTool<'a> {
    tool: Tool,
    name: &'static str,
    cost: f64,
    tokens: Tokens,
    total_tokens: u64,
    sessions: usize,
    responses: u64,
    unpriced_models: &'a [String],
    rows: &'a [Row],
    #[serde(skip_serializing_if = "Option::is_none")]
    five_hour_window: Option<&'a Block>,
}
