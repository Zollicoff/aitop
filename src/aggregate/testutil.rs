//! Helpers for aggregate tests.

use crate::event::{Event, Interner, Tokens, Tool};

pub const DAY: i64 = 24 * super::range::HOUR;

/// An event for `project` (a path) and `session`, costing `cost`.
pub fn ev(i: &mut Interner, tool: Tool, ts: i64, project: &str, session: &str, cost: f64) -> Event {
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
