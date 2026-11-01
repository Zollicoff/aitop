//! `aitop pricing`: show, look up or update model prices.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Result};

use super::Context;
use crate::event::Tool;
use crate::format;
use crate::paths::tilde;
use crate::pricing::{self, PriceTable};

/// Download current prices into the cache (or `output`).
pub fn update(output: Option<&Path>) -> Result<()> {
    let (path, n) = pricing::update(output)?;
    println!("saved {n} model prices to {}", tilde(&path));
    Ok(())
}

/// Print the price used for one model id.
pub fn show(ctx: &Context, model: &str) -> Result<()> {
    let table = PriceTable::load(&ctx.cfg.pricing);
    let Some((key, p)) = table.lookup(model) else {
        bail!("no price known for {model:?}; add one under [pricing] in the config");
    };
    println!("{model} → {key}");
    println!("{}", serde_json::to_string_pretty(p)?);
    Ok(())
}

/// Every model seen in the logs with the price applied to it.
pub fn overview(ctx: &Context) -> Result<()> {
    let c = ctx.snapshot();
    let table = c.pricing();
    let d = c.data.read().unwrap();
    let mut seen: BTreeMap<(Tool, &str), f64> = BTreeMap::new();
    for e in &d.events {
        *seen.entry((e.tool, &e.model)).or_default() += e.cost;
    }
    println!("prices: {}  (USD per million tokens)\n", table.sources.join(" + "));
    println!(
        "{:<8} {:<34} {:<26} {:>7} {:>7} {:>7} {:>7} {:>9}",
        "TOOL", "MODEL", "PRICED AS", "INPUT", "OUTPUT", "C.READ", "C.WRITE", "COST"
    );
    let opt = |v: Option<f64>| v.map(|v| v.to_string()).unwrap_or_else(|| "-".into());
    for ((tool, model), cost) in &seen {
        let model_col = format::truncate(model, 34);
        match table.lookup(model) {
            Some((key, p)) => println!(
                "{:<8} {model_col:<34} {:<26} {:>7} {:>7} {:>7} {:>7} {:>9}",
                tool.key(),
                format::truncate(key, 26),
                p.input,
                p.output,
                opt(p.cache_read),
                opt(p.cache_write),
                format::cost(*cost)
            ),
            None => println!("{:<8} {model_col:<34} (no price)", tool.key()),
        }
    }
    if seen.is_empty() {
        println!("(no usage found yet)");
    }
    println!("\n{} models in the price table. Update with `aitop pricing update`.", table.len());
    Ok(())
}
