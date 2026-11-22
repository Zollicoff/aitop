//! Turning T3 Code's daily totals into usage records.

use super::client::{DayTotal, Machine};
use super::usage;
use crate::event::{Event, Interner, Rollup};
use crate::host::normalize;

/// The machine a total was read on, as recorded in its event: `t3:<host>`.
pub fn read_on(e: &Event) -> Option<&str> {
    e.rollup.and(e.session.strip_prefix("t3:"))
}

/// A linked machine's totals as usage records filed under its name.
pub fn events(machine: &Machine, totals: &[DayTotal], interner: &mut Interner, costs: bool) -> Vec<Event> {
    records(&machine.name, &machine.id, totals, interner, costs)
}

/// This machine's other-agent totals as usage records filed under `host`.
pub fn local_events(host: &str, totals: &[DayTotal], interner: &mut Interner, costs: bool) -> Vec<Event> {
    records(host, "local", totals, interner, costs)
}

/// Totals as usage records for the machine called `name`.
///
/// Each is one record per agent, model and day, marked as a [`Rollup`]. Its
/// id is stable, so a day that is read again replaces what was held. With
/// `costs`, a record keeps the cost T3 reported; otherwise (and where T3 had
/// no price at all) it is priced here from its token totals.
///
/// Claude Code and Codex totals appear as one project row, "name · via T3
/// Code". The other agents share a panel, so there the row is the agent.
fn records(name: &str, scope: &str, totals: &[DayTotal], interner: &mut Interner, costs: bool) -> Vec<Event> {
    let host = interner.get(name);
    let via = (interner.get(&format!("{name} · via T3 Code")), interner.get(&format!("t3://{name}")));
    totals
        .iter()
        .map(|d| {
            let t = &d.total;
            let read_on = normalize(&t.host);
            let (id, (project, project_path)) = if t.tool.has_reader() {
                (format!("t3:{scope}:{read_on}:{}:{}:{}", t.tool.key(), t.model, t.day), via.clone())
            } else {
                // Not tied to the link it came over: the same usage reached
                // two ways is one record.
                let agent =
                    (interner.get(usage::agent_name(&t.agent)), interner.get(&format!("t3-agent://{}", t.agent)));
                (format!("t3:other:{read_on}:{}:{}:{}", t.agent, t.model, t.day), agent)
            };
            Event {
                tool: t.tool,
                host: host.clone(),
                id: interner.get(&id),
                ts: d.ts,
                session: interner.get(&format!("t3:{read_on}")),
                project,
                project_path,
                model: interner.get(&t.model),
                tokens: t.tokens,
                speed: crate::event::Speed::Standard,
                cost: 0.0,
                priced: false,
                rollup: Some(Rollup {
                    responses: t.responses,
                    sessions: t.sessions,
                    cost: (costs && t.unpriced < t.responses).then_some(t.cost),
                    unpriced: costs && t.unpriced > 0 && t.unpriced < t.responses,
                }),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Tool;
    use crate::t3::client::Zone;

    #[test]
    fn totals_become_records() {
        let zone = Zone { name: "UTC".into(), local: false };
        let mut totals: Vec<DayTotal> = usage::parse(&usage::tests::sample("Studio-PC.local"))
            .totals
            .into_iter()
            .map(|total| DayTotal { ts: zone.day_start(&total.day).unwrap(), total })
            .collect();
        let machine = Machine { id: "env-1".into(), name: "studio-pc".into(), result: Ok(Vec::new()) };
        let mut i = Interner::default();
        totals[1].total.unpriced = totals[1].total.responses;
        let events = events(&machine, &totals, &mut i, true);
        assert_eq!(events.len(), 3);
        let grok = &events[2];
        assert_eq!((grok.tool, &*grok.project, &*grok.project_path), (Tool::Grok, "Grok Build", "t3-agent://grok"));
        assert_eq!(&*grok.id, "t3:other:studio-pc:grok:grok-build:2026-11-15");
        let here = local_events("desk", &totals[2..], &mut i, true);
        assert_eq!((&*here[0].host, &here[0].id), ("desk", &grok.id), "the same usage however it was reached");
        let e = &events[0];
        assert_eq!(&*e.id, "t3:env-1:studio-pc:claude:claude-haiku-5-5:2026-11-14");
        assert_eq!(
            (&*e.host, &*e.project, &*e.project_path),
            ("studio-pc", "studio-pc · via T3 Code", "t3://studio-pc")
        );
        assert_eq!(e.rollup.map(|r| (r.responses, r.sessions, r.unpriced)), Some((20, 4, false)));
        assert!((e.rollup.unwrap().cost.unwrap() - 0.0444).abs() < 1e-9, "T3's cost is kept");
        assert_eq!(events[1].rollup.unwrap().cost, None, "T3 had no price: priced here");
        assert!(super::events(&machine, &totals, &mut i, false).iter().all(|e| e.rollup.unwrap().cost.is_none()));
        assert_eq!((e.responses(), e.ts, e.tokens.input), (20, 1_794_614_400_000, 40));
        assert_eq!(read_on(e), Some("studio-pc"));
        assert_eq!(read_on(&Event { rollup: None, ..e.clone() }), None, "only totals say where they were read");
    }
}
