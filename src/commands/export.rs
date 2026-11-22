//! `aitop export`: this machine's usage records, one per line.
//!
//! This is what another machine's aitop runs here (over ssh) when this
//! machine is one of its `[[remote]]`s. Only usage made on this machine is
//! printed, never what it pulled from others, and nothing is written.

use std::io::{BufWriter, Write};

use anyhow::Result;

use super::Context;
use crate::event::Event;
use crate::remote;

#[derive(clap::Args)]
pub struct Args {
    /// Only records made at or after this time (unix milliseconds).
    #[arg(long, value_name = "MS")]
    pub since: Option<i64>,
}

pub fn run(ctx: &Context, args: &Args) -> Result<()> {
    let mut c = ctx.collector();
    // Another aitop may be running here and appending to the history.
    c.read_only();
    if let Some(since) = args.since {
        c.read_since(since);
    }
    c.load_history();
    c.scan();
    let d = c.data.read().unwrap();
    let host = c.host();
    let since = args.since.unwrap_or(i64::MIN);
    let own: Vec<&Event> = d.events.iter().filter(|e| e.host == *host && e.ts >= since).collect();
    let mut out = BufWriter::new(std::io::stdout().lock());
    match remote::write_export(&mut out, host, &own).and_then(|()| out.flush()) {
        // The reader went away (`aitop export | head`): nothing to report.
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => Ok(other?),
    }
}
