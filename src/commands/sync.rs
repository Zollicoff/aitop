//! `aitop sync`: read your other machines now.
//!
//! The dashboard does this by itself while it runs. This is for checking
//! that a machine can be read, and for keeping `aitop report` current from
//! cron. It covers both ways aitop knows other machines: the ones linked in
//! T3 Code, and the `[[remote]]`s in the config.

use std::collections::HashMap;

use anyhow::{bail, Result};
use chrono::Local;

use super::Context;
use crate::format;
use crate::paths::tilde;

pub fn run(ctx: &Context) -> Result<()> {
    let mut c = ctx.collector();
    c.load_history();
    // This machine's own usage first, so a response both machines can see stays here.
    c.scan();
    let mut held: HashMap<String, usize> = HashMap::new();
    for e in &c.data.read().unwrap().events {
        *held.entry(e.host.to_string()).or_default() += 1;
    }
    let machines = c.sync_remotes();
    let d = c.data.read().unwrap();
    match &d.t3 {
        Some(t3) => match &t3.error {
            None => println!("T3 Code {}  read", t3.version),
            Some(e) => println!("T3 Code {}  not read: {e}", t3.version),
        },
        None if ctx.cfg.t3.enabled => println!("T3 Code  not found on this machine"),
        None => println!("T3 Code  turned off in the config"),
    }
    if machines.is_empty() {
        println!(
            "No other machines. Link them in T3 Code (Settings → Environments), or list them under [[remote]] in {}.",
            tilde(&ctx.config_path)
        );
        return Ok(());
    }
    let now = Local::now().timestamp_millis();
    let mut failed = 0;
    for r in &machines {
        let had = held.get(&r.name).copied().unwrap_or(0);
        match &r.error {
            None => {
                let newest = r.newest.map(|t| format!(", newest {} ago", format::duration(now - t)));
                println!(
                    "✓ {}  ({})  {} new · {} records{}",
                    r.name,
                    r.target,
                    format::thousands(r.records.saturating_sub(had) as u64),
                    format::thousands(r.records as u64),
                    newest.unwrap_or_default()
                );
            }
            Some(e) => {
                failed += 1;
                println!("✗ {}  ({})  {e}", r.name, r.target);
                if r.records > 0 {
                    println!("    still showing the {} records it sent before", format::thousands(r.records as u64));
                }
            }
        }
    }
    if ctx.history.is_none() {
        println!("History is off, so nothing was kept; the dashboard reads them again each time it starts.");
    }
    if failed > 0 {
        bail!("{failed} machine{} could not be read", if failed == 1 { "" } else { "s" });
    }
    Ok(())
}
