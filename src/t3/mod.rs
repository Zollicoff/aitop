//! T3 Code as a source of usage.
//!
//! [T3 Code](https://github.com/pingdotgg/t3code) runs Claude Code and Codex
//! and keeps its own usage view. When it is installed and running, aitop
//! asks it for two things, with nothing to set up:
//!
//! * **the transcript folders it uses on this machine**, which aitop then
//!   reads itself (T3 gives its tools their own config folders, which aitop
//!   would not otherwise find);
//! * **the usage of the machines linked to it** (T3's Linked environments),
//!   as a total per tool, model and day. T3 does not share projects or
//!   sessions, so those machines show less detail than this one;
//! * **the usage of agents aitop has no reader for** (Grok, Cursor, Pi, …),
//!   on this machine and the linked ones, as the same kind of totals. Each
//!   has its own panel; one this aitop does not know by name goes in
//!   [`crate::event::Tool::Other`].
//!
//! How it gets in: aitop runs T3's own `t3 auth session issue` to give
//! itself a read-only session on the local server, kept in
//! `<data dir>/t3.json`. For a linked machine it asks the local server to
//! pair it (`peerLinks.pairClient`, which needs a short-lived session that
//! may manage access), redeems the pairing at that machine for a session
//! that can only read usage, and asks it directly from then on. Every
//! session is listed in T3 under Sharing & access as `aitop`, where it can
//! be revoked.

mod client;
mod install;
mod records;
mod rpc;
mod state;
mod usage;

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use serde::Deserialize;

pub use client::{Client, Outcome};
// Named only where they are built by hand: the collector's tests.
#[cfg(test)]
pub use client::{DayTotal, Machine};
pub use install::{rates_path, Install};
pub use records::{events, local_events, read_on};
pub use state::{cached_roots, state_path};
#[cfg(test)]
pub use usage::Total;

/// The `[t3]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct T3Config {
    /// Read from T3 Code when it is installed and running.
    pub enabled: bool,
    /// Seconds between reads.
    pub every: f64,
    /// Use T3 Code's prices: its price list for this machine's usage, and
    /// the cost it reports for linked machines.
    pub prices: bool,
}

impl Default for T3Config {
    fn default() -> Self {
        T3Config { enabled: true, every: 300.0, prices: true }
    }
}

impl T3Config {
    pub fn use_prices(&self) -> bool {
        self.enabled && self.prices
    }

    pub fn interval(&self) -> Duration {
        Duration::from_secs_f64(self.every.clamp(30.0, 86_400.0))
    }
}

/// Read T3 on its own thread now, then every `every` and whenever the
/// returned sender is sent to. Stops when either channel closes.
pub fn spawn(mut client: Client, every: Duration, out: Sender<Outcome>) -> Sender<()> {
    let (wake, woken) = mpsc::channel();
    let spawned = std::thread::Builder::new().name("aitop-t3".into()).spawn(move || loop {
        if out.send(client.sync()).is_err() {
            break;
        }
        match woken.recv_timeout(every) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    });
    drop(spawned);
    wake
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config() {
        let c = T3Config::default();
        assert!(c.enabled);
        assert_eq!(c.interval(), Duration::from_secs(300));
        assert!(c.use_prices());
        assert!(!T3Config { enabled: false, ..c.clone() }.use_prices());
        assert_eq!(T3Config { every: 1.0, ..c }.interval(), Duration::from_secs(30));
    }
}
