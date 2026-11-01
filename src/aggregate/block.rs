//! Claude's 5-hour usage windows.

use serde::Serialize;

use super::range::HOUR;
use crate::event::Event;

/// Claude subscription usage is metered in rolling 5-hour windows.
pub const BLOCK: i64 = 5 * HOUR;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Block {
    pub start: i64,
    pub end: i64,
    pub cost: f64,
    pub tokens: u64,
    pub messages: u64,
    pub burn_per_hour: f64,
    pub projected: f64,
    pub active: bool,
}

/// Group events into 5-hour windows (a window starts at the hour of the first
/// message after the previous one expired) and return the latest.
pub fn current_block<'a>(events: impl Iterator<Item = &'a Event>, now: i64) -> Option<Block> {
    let mut block: Option<Block> = None;
    let mut first_ts = 0;
    for e in events {
        let b = match &mut block {
            Some(b) if e.ts < b.end => b,
            _ => {
                let start = e.ts - e.ts.rem_euclid(HOUR);
                first_ts = e.ts;
                block.insert(Block { start, end: start + BLOCK, ..Default::default() })
            }
        };
        b.cost += e.cost;
        b.tokens += e.tokens.total();
        b.messages += 1;
    }
    let mut b = block?;
    b.active = (b.start..b.end).contains(&now);
    // Burn rate since the window's first message (at least 10 minutes, so a
    // single early message doesn't extrapolate wildly).
    let elapsed_h = (now.min(b.end) - first_ts).max(10 * 60_000) as f64 / HOUR as f64;
    b.burn_per_hour = b.cost / elapsed_h;
    b.projected = if b.active { b.cost + b.burn_per_hour * (b.end - now) as f64 / HOUR as f64 } else { b.cost };
    Some(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::testutil::ev;
    use crate::event::{Interner, Tool};

    #[test]
    fn five_hour_blocks() {
        let mut i = Interner::default();
        let h = |x: f64| (x * HOUR as f64) as i64;
        let base = 1_800_000_000_000 - 1_800_000_000_000 % HOUR;
        let events = [
            ev(&mut i, Tool::Claude, base + h(0.5), "/p", "s", 1.0),
            ev(&mut i, Tool::Claude, base + h(4.9), "/p", "s", 1.0),
            ev(&mut i, Tool::Claude, base + h(5.2), "/p", "s", 2.0), // new block at base+5h
            ev(&mut i, Tool::Claude, base + h(6.0), "/p", "s", 2.0),
        ];
        let b = current_block(events.iter(), base + h(7.2)).unwrap();
        assert_eq!(b.start, base + h(5.0));
        assert_eq!(b.end, base + h(10.0));
        assert_eq!(b.cost, 4.0);
        assert!(b.active);
        assert!((b.burn_per_hour - 4.0 / 2.0).abs() < 1e-9);
        assert!((b.projected - (4.0 + 2.0 * 2.8)).abs() < 1e-6);
        let later = current_block(events.iter(), base + h(11.0)).unwrap();
        assert!(!later.active);
        assert!(current_block([].iter(), 0).is_none());
    }
}
