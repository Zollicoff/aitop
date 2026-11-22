//! What aitop remembers about T3 Code between runs: its sessions and the
//! folders T3 reported.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::event::Tool;

/// How much of a machine's history has been read: bumped when a read learns
/// to keep more (costs, then other agents), so the history is read again.
pub(super) const LEVEL: u8 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct Session {
    /// T3's id for the session, for revoking it (empty when not known).
    #[serde(default)]
    pub(super) id: String,
    pub(super) token: String,
    /// Unix ms.
    pub(super) expires: i64,
}

impl Session {
    /// Still good for a while: one about to expire is replaced early.
    pub(super) fn usable(&self, now: i64) -> bool {
        self.expires - now > 10 * 60_000
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct Peer {
    pub(super) label: String,
    /// The address the session was made at.
    pub(super) url: String,
    pub(super) session: Session,
    /// Its history has been read in full at this [`LEVEL`]; later reads
    /// only go back a couple of days.
    #[serde(default)]
    pub(super) level: u8,
}

/// What aitop remembers between runs, in `<data dir>/t3.json` (it holds
/// tokens, so it is only readable by the user).
#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub(super) struct State {
    /// The server the sessions below belong to.
    pub(super) origin: String,
    pub(super) local: Option<Session>,
    /// Sessions on linked machines, by T3 environment id.
    pub(super) peers: BTreeMap<String, Peer>,
    /// Transcript folders T3 reported for this machine, by tool key.
    pub(super) roots: BTreeMap<String, Vec<PathBuf>>,
    /// This machine's other agents: the [`LEVEL`] their history was read
    /// at, and the day of the last read.
    pub(super) local_level: u8,
    pub(super) local_day: Option<String>,
}

pub fn state_path() -> PathBuf {
    crate::paths::data_dir().join("t3.json")
}

impl State {
    pub(super) fn load(path: &Path) -> State {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub(super) fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&tmp)?;
        serde_json::to_writer_pretty(&file, self)?;
        drop(file);
        std::fs::rename(&tmp, path)
    }
}

/// Transcript folders T3 reported on an earlier run, for commands that do
/// not talk to T3 themselves.
pub fn cached_roots(path: &Path) -> Vec<(Tool, PathBuf)> {
    let state = State::load(path);
    Tool::ALL
        .into_iter()
        .flat_map(|t| state.roots.get(t.key()).cloned().unwrap_or_default().into_iter().map(move |p| (t, p)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_privately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/t3.json");
        assert_eq!(State::load(&path), State::default());
        let session = Session { id: "s1".into(), token: "tok".into(), expires: 5_000_000 };
        let mut state =
            State { origin: "http://127.0.0.1:3773".into(), local: Some(session.clone()), ..State::default() };
        let peer = Peer { label: "laptop".into(), url: "https://laptop".into(), session, level: LEVEL };
        state.peers.insert("env".into(), peer);
        state.roots.insert("claude".into(), vec!["/home/z/.claude-alt/projects".into()]);
        state.save(&path).unwrap();
        assert_eq!(State::load(&path), state);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(cached_roots(&path), [(Tool::Claude, PathBuf::from("/home/z/.claude-alt/projects"))]);
        assert!(state.local.as_ref().unwrap().usable(0));
        assert!(!state.local.as_ref().unwrap().usable(5_000_000 - 60_000), "about to expire");
    }
}
