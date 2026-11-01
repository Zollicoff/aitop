//! `aitop doctor`: where aitop looks for logs and what it found.

use std::time::Instant;

use anyhow::Result;
use chrono::Local;

use super::Context;
use crate::format;
use crate::paths::tilde;

pub fn run(ctx: &Context) -> Result<()> {
    println!("aitop {}", env!("CARGO_PKG_VERSION"));
    let found = if ctx.config_path.exists() { "found" } else { "not present, using defaults" };
    println!("config   {} ({found})", tilde(&ctx.config_path));
    println!("history  {}", ctx.history.as_deref().map(tilde).unwrap_or_else(|| "disabled".into()));

    let started = Instant::now();
    let c = ctx.snapshot();
    let elapsed = started.elapsed();
    let d = c.data.read().unwrap();
    println!("pricing  {}\n", d.pricing_sources.join(" + "));

    let now = Local::now().timestamp_millis();
    for (i, st) in d.status.iter().enumerate() {
        let n = d.events.iter().filter(|e| e.tool == st.tool).count();
        println!("{}  ({} files, {} records)", st.tool.name(), st.files, n);
        for (p, exists) in &st.roots {
            println!("  {} {}", if *exists { "✓" } else { "✗" }, tilde(p));
        }
        for e in st.errors.iter().take(5) {
            println!("  ! {e}");
        }
        if let Some(Some(l)) = d.limits.get(i) {
            println!("  rate limits as of {} ago:", format::duration(now - l.ts));
            for w in [&l.primary, &l.secondary].into_iter().flatten() {
                println!("    {:>5.1}% of {} min window", w.used_percent, w.window_minutes.unwrap_or(0));
            }
        }
    }
    if let Some(e) = &d.history_error {
        println!("\nhistory error: {e}");
    }
    println!("\nscan took {} ms", elapsed.as_millis());
    Ok(())
}
