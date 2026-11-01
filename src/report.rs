//! Plain-text and JSON reports (`aitop report`).

use chrono::Local;
use serde::Serialize;

use crate::aggregate::{daily, Range, Row, Summary};
use crate::event::{Event, Tool};
use crate::format;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum By {
    Project,
    Model,
    Session,
    Day,
}

#[derive(Serialize)]
struct JsonTool<'a> {
    tool: Tool,
    name: &'static str,
    cost: f64,
    tokens: crate::event::Tokens,
    total_tokens: u64,
    sessions: usize,
    responses: u64,
    unpriced_models: &'a [String],
    rows: Vec<Row>,
    #[serde(skip_serializing_if = "Option::is_none")]
    five_hour_window: Option<&'a crate::aggregate::Block>,
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

fn rows_for(summary: &Summary, events: &[Event], tool: Tool, by: By) -> Vec<Row> {
    let ts = summary.tool(tool);
    match by {
        By::Project => ts.projects.clone(),
        By::Model => ts.models.clone(),
        By::Session => ts.session_rows.clone(),
        By::Day => daily(events, Some(tool), summary.range, Local::now()),
    }
}

pub fn render(summary: &Summary, events: &[Event], by: By, only: Option<Tool>, json: bool, limit: usize) -> String {
    let tools: Vec<Tool> = Tool::ALL.into_iter().filter(|t| only.is_none_or(|o| o == *t)).collect();
    if json {
        let report = JsonReport {
            range: summary.range,
            generated_at: Local::now().to_rfc3339(),
            by: match by {
                By::Project => "project",
                By::Model => "model",
                By::Session => "session",
                By::Day => "day",
            },
            total_cost: tools.iter().map(|&t| summary.tool(t).cost).sum(),
            total_tokens: tools.iter().map(|&t| summary.tool(t).tokens.total()).sum(),
            tools: tools
                .iter()
                .map(|&t| {
                    let ts = summary.tool(t);
                    JsonTool {
                        tool: t,
                        name: t.name(),
                        cost: ts.cost,
                        tokens: ts.tokens,
                        total_tokens: ts.tokens.total(),
                        sessions: ts.sessions,
                        responses: ts.messages,
                        unpriced_models: &ts.unpriced_models,
                        rows: rows_for(summary, events, t, by),
                        five_hour_window: ts.block.as_ref().filter(|b| b.active),
                    }
                })
                .collect(),
        };
        return serde_json::to_string_pretty(&report).expect("serializable") + "\n";
    }

    let mut out = String::new();
    let total: f64 = tools.iter().map(|&t| summary.tool(t).cost).sum();
    let total_tokens: u64 = tools.iter().map(|&t| summary.tool(t).tokens.total()).sum();
    out += &format!(
        "aitop · {} · {} · {} tokens\n",
        summary.range.label(),
        format::cost(total),
        format::tokens(total_tokens)
    );
    let name_h = match by {
        By::Project => "PROJECT",
        By::Model => "MODEL",
        By::Session => "SESSION",
        By::Day => "DAY",
    };
    for t in tools {
        let ts = summary.tool(t);
        out += &format!(
            "\n{}  {} · {} tokens · {} sessions · {} responses\n",
            t.name(),
            format::cost(ts.cost),
            format::tokens(ts.tokens.total()),
            format::thousands(ts.sessions as u64),
            format::thousands(ts.messages)
        );
        let rows = rows_for(summary, events, t, by);
        if rows.is_empty() {
            out += "  (no usage)\n";
            continue;
        }
        out += &format!(
            "  {:<34} {:>8} {:>9} {:>9} {:>9} {:>10}\n",
            name_h, "SESSIONS", "INPUT", "OUTPUT", "CACHE", "COST"
        );
        for r in rows.iter().take(limit) {
            let name = match by {
                By::Session => format!("{} {}", r.name, r.key.chars().take(8).collect::<String>()),
                By::Model => format::short_model(&r.name),
                _ => r.name.clone(),
            };
            let cost = if r.unpriced { format!("{}*", format::cost(r.cost)) } else { format::cost(r.cost) };
            out += &format!(
                "  {:<34} {:>8} {:>9} {:>9} {:>9} {:>10}\n",
                format::truncate(&name, 34),
                r.sessions,
                format::tokens(r.tokens.input),
                format::tokens(r.tokens.output),
                format::tokens(r.tokens.cache_read + r.tokens.cache_writes()),
                cost
            );
        }
        if rows.len() > limit {
            out += &format!("  … {} more (use --limit)\n", rows.len() - limit);
        }
        if !ts.unpriced_models.is_empty() {
            out += &format!("  * no price for: {}\n", ts.unpriced_models.join(", "));
        }
    }
    out
}
