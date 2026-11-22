//! Usage from other machines.
//!
//! Each `[[remote]]` in the config names a machine that also runs aitop.
//! Every so often its `aitop export` is run there (over ssh by default) and
//! the records it prints are merged in under that machine's name. They are
//! kept in the local history like any other record, so a machine that is
//! asleep or unreachable still shows what it last reported.
//!
//! Records keep their ids across machines, and the collector drops ids it
//! has already seen, so a response visible from two machines (a synced
//! transcript folder, say) is counted once.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::event::{Event, Interner};
use crate::store;

/// Re-read this much before the newest record already held: a response still
/// streaming at the last sync is reported again once it has finished.
const OVERLAP_MS: i64 = 10 * 60_000;
const DEFAULT_EVERY: f64 = 60.0;
/// Where aitop usually lives; a non-interactive ssh session has a bare PATH.
const REMOTE_PATH: &str = "$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin";

/// One `[[remote]]` table.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteConfig {
    /// A host that `ssh` reaches without asking for a password.
    pub ssh: Option<String>,
    /// Instead of `ssh`: a command that runs aitop on the other machine.
    /// `export` and its flags are added to it.
    pub command: Vec<String>,
    /// What to call the machine (default: the ssh host).
    pub name: Option<String>,
    /// aitop's path on the other machine, if it is not on the PATH.
    pub aitop: Option<String>,
    /// Seconds between syncs.
    pub every: Option<f64>,
}

/// A machine to pull usage from.
#[derive(Debug, Clone)]
pub struct Remote {
    /// The machine's name in the dashboard.
    pub name: String,
    /// How it is reached, for display.
    pub target: String,
    via: Via,
    pub every: Duration,
}

#[derive(Debug, Clone)]
enum Via {
    Ssh { host: String, aitop: Option<String> },
    Command(Vec<String>),
}

/// The first line `aitop export` prints.
#[derive(Serialize, Deserialize)]
pub struct Header {
    /// Version of the aitop that answered.
    pub aitop: String,
    /// What that machine calls itself.
    pub host: String,
    /// Its system hostname, which is what other programs know it by.
    #[serde(default)]
    pub hostname: String,
}

/// What one sync returned.
pub struct Batch {
    pub header: Header,
    pub events: Vec<Event>,
}

/// The remotes in `cfg`, or what is wrong with them. `local` is this
/// machine's name, which no remote may share.
pub fn resolve(cfg: &Config, local: &str) -> Result<Vec<Remote>> {
    let mut out: Vec<Remote> = Vec::new();
    for (i, c) in cfg.remote.iter().enumerate() {
        let r = Remote::from_config(c).map_err(|e| anyhow::anyhow!("[[remote]] #{}: {e}", i + 1))?;
        if r.name == local {
            bail!(
                "[[remote]] #{}: `{}` is this machine's own name; set `name` (or `host`) to tell them apart",
                i + 1,
                r.name
            );
        }
        if out.iter().any(|o| o.name == r.name) {
            bail!("[[remote]] #{}: there is already a remote called `{}`", i + 1, r.name);
        }
        out.push(r);
    }
    Ok(out)
}

impl Remote {
    fn from_config(c: &RemoteConfig) -> Result<Remote> {
        let every = c.every.unwrap_or(DEFAULT_EVERY);
        if !(5.0..=86_400.0).contains(&every) {
            bail!("`every` must be between 5 and 86400 seconds");
        }
        let name = c.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
        let (via, target, default_name) = match (c.ssh.as_deref().map(str::trim), c.command.is_empty()) {
            (Some(_), false) => bail!("set either `ssh` or `command`, not both"),
            (None, true) => bail!("needs `ssh = \"host\"` (or `command`)"),
            (Some(host), true) => {
                if host.is_empty() || host.starts_with('-') || host.contains(char::is_whitespace) {
                    bail!("`ssh` must be a host name, alias or user@host");
                }
                let bare = host.rsplit('@').next().unwrap_or(host).to_lowercase();
                (Via::Ssh { host: host.to_string(), aitop: c.aitop.clone() }, format!("ssh {host}"), Some(bare))
            }
            (None, false) => {
                if c.aitop.is_some() {
                    bail!("`aitop` only applies to `ssh`; put the path in `command`");
                }
                (Via::Command(c.command.clone()), c.command.join(" "), None)
            }
        };
        let Some(name) = name.map(String::from).or(default_name) else { bail!("a `command` remote needs a `name`") };
        Ok(Remote { name, target, via, every: Duration::from_secs_f64(every) })
    }

    /// The command that prints the remote's records made at or after `since`.
    fn command(&self, since: Option<i64>) -> Command {
        let mut export = vec!["export".to_string()];
        if let Some(s) = since {
            export.extend(["--since".to_string(), s.to_string()]);
        }
        match &self.via {
            Via::Ssh { host, aitop } => {
                let program = match aitop {
                    Some(path) => shell_quote(path),
                    None => format!("PATH=\"{REMOTE_PATH}:$PATH\" aitop"),
                };
                let mut c = Command::new("ssh");
                // Never prompt, and give up on a machine that is asleep.
                for opt in ["BatchMode=yes", "ConnectTimeout=10", "ServerAliveInterval=5", "ServerAliveCountMax=3"] {
                    c.args(["-o", opt]);
                }
                c.arg(host).arg(format!("{program} {}", export.join(" ")));
                c
            }
            Via::Command(argv) => {
                let mut c = Command::new(&argv[0]);
                c.args(&argv[1..]).args(&export);
                c
            }
        }
    }

    /// Run the export and return what it printed.
    pub fn fetch(&self, since: Option<i64>) -> Result<Vec<u8>, String> {
        let out = self
            .command(since)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("cannot run `{}`: {e}", self.target))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let why = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no error message").trim();
            return Err(format!("{} ({})", why, out.status));
        }
        Ok(out.stdout)
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The start of a sync that already holds records up to `newest`.
pub fn since(newest: Option<i64>) -> Option<i64> {
    newest.map(|ts| ts - OVERLAP_MS)
}

/// What `aitop export` prints: a [`Header`], then one record per line.
pub fn write_export(out: &mut impl std::io::Write, host: &str, events: &[&Event]) -> std::io::Result<()> {
    let header =
        Header { aitop: env!("CARGO_PKG_VERSION").into(), host: host.into(), hostname: crate::host::local(None) };
    writeln!(out, "{}", serde_json::to_string(&header).expect("header serializes"))?;
    for e in events {
        writeln!(out, "{}", store::encode(e))?;
    }
    Ok(())
}

/// Parse the output of `aitop export`, filing every record under `name`.
pub fn parse(output: &[u8], interner: &mut Interner, name: &Arc<str>) -> Result<Batch, String> {
    let text = String::from_utf8_lossy(output);
    let mut lines = text.lines();
    // A login shell may print a greeting first; skip to the header.
    let header = lines
        .by_ref()
        .find_map(|l| serde_json::from_str::<Header>(l).ok())
        .ok_or("no answer from `aitop export` (is an aitop that has `export` installed there?)")?;
    let events = lines
        .filter_map(|l| store::decode(l, interner, name))
        .map(|mut e| {
            e.host = name.clone();
            e
        })
        .collect();
    Ok(Batch { header, events })
}

/// The result of one background sync, for the collector to merge.
pub struct Fetched {
    /// Position of the remote in the collector's list.
    pub index: usize,
    pub result: Result<Vec<u8>, String>,
}

/// Timestamp of the newest record held for a remote; `NONE` before the first.
pub type Cursor = Arc<AtomicI64>;
pub const NONE: i64 = i64::MIN;

/// Sync `remote` on its own thread now, then every `remote.every` and
/// whenever the returned sender is sent to. Stops when either channel closes.
pub fn spawn(index: usize, remote: Remote, cursor: Cursor, out: Sender<Fetched>) -> Sender<()> {
    let (wake, woken) = mpsc::channel();
    let spawned = std::thread::Builder::new().name(format!("aitop-remote-{}", remote.name)).spawn(move || loop {
        let newest = Some(cursor.load(Ordering::Relaxed)).filter(|&c| c != NONE);
        let result = remote.fetch(since(newest));
        if out.send(Fetched { index, result }).is_err() {
            break;
        }
        match woken.recv_timeout(remote.every) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    });
    // Without a thread the remote simply never reports.
    drop(spawned);
    wake
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::testutil::ev;
    use crate::event::Tool;

    fn config(toml: &str) -> Config {
        toml::from_str(toml).unwrap()
    }

    #[test]
    fn resolves_remotes() {
        let cfg = config(
            "[[remote]]\nssh = \"sam@MBP\"\n\n[[remote]]\ncommand = [\"docker\", \"exec\", \"box\", \"aitop\"]\nname = \"box\"\nevery = 300\n",
        );
        let r = resolve(&cfg, "mini").unwrap();
        assert_eq!((r[0].name.as_str(), r[0].target.as_str()), ("mbp", "ssh sam@MBP"));
        assert_eq!((r[1].name.as_str(), r[1].every), ("box", Duration::from_secs(300)));
        let args = |r: &Remote, s| {
            let c = r.command(s);
            c.get_args().map(|a| a.to_string_lossy().to_string()).collect::<Vec<_>>()
        };
        let ssh = args(&r[0], Some(42));
        assert_eq!(ssh[ssh.len() - 2], "sam@MBP");
        assert!(ssh.last().unwrap().ends_with(" aitop export --since 42"), "{ssh:?}");
        assert_eq!(args(&r[1], None), ["exec", "box", "aitop", "export"]);

        for (bad, why) in [
            ("[[remote]]\nname = \"x\"\n", "needs `ssh"),
            ("[[remote]]\nssh = \"-oProxyCommand=x\"\n", "must be a host"),
            ("[[remote]]\nssh = \"a\"\ncommand = [\"b\"]\n", "not both"),
            ("[[remote]]\ncommand = [\"b\"]\n", "needs a `name`"),
            ("[[remote]]\nssh = \"mini\"\n", "own name"),
            ("[[remote]]\nssh = \"a\"\n[[remote]]\nssh = \"u@a\"\n", "already a remote"),
            ("[[remote]]\nssh = \"a\"\nevery = 1\n", "between 5"),
        ] {
            let err = resolve(&config(bad), "mini").unwrap_err().to_string();
            assert!(err.contains(why), "{bad:?} gave {err:?}");
        }
        assert_eq!(shell_quote("/opt/it's/aitop"), "'/opt/it'\\''s/aitop'");
    }

    #[test]
    fn export_round_trips_under_the_remote_name() {
        let mut i = Interner::default();
        let a = ev(&mut i, Tool::Claude, 1000, "/code/web", "s1", 1.0);
        let b = ev(&mut i, Tool::Codex, 2000, "/code/api", "s2", 2.0);
        let mut out = b"Welcome to the laptop\n".to_vec();
        write_export(&mut out, "sams-laptop", &[&a, &b]).unwrap();
        out.extend(b"not a record\n");
        let batch = parse(&out, &mut i, &Arc::from("mbp")).unwrap();
        assert_eq!(batch.header.host, "sams-laptop");
        assert_eq!(batch.events.len(), 2);
        assert!(batch.events.iter().all(|e| &*e.host == "mbp"), "filed under the configured name");
        assert_eq!((batch.events[1].ts, &*batch.events[1].project_path), (2000, "/code/api"));
        assert_eq!(batch.events[0].id, a.id);
        assert!(parse(b"command not found: aitop\n", &mut i, &Arc::from("mbp")).is_err());
        assert_eq!(since(Some(OVERLAP_MS + 5)), Some(5));
        assert_eq!(since(None), None);
    }
}
