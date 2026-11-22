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
//!   on this machine and the linked ones, as the same kind of totals. They
//!   share one panel, [`Tool::Other`].
//!
//! How it gets in: aitop runs T3's own `t3 auth session issue` to give
//! itself a read-only session on the local server, kept in
//! `<data dir>/t3.json`. For a linked machine it asks the local server to
//! pair it (`peerLinks.pairClient`, which needs a short-lived session that
//! may manage access), redeems the pairing at that machine for a session
//! that can only read usage, and asks it directly from then on. Every
//! session is listed in T3 under Sharing & access as `aitop`, where it can
//! be revoked.

mod rpc;
mod usage;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use chrono::{Days, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::event::{Event, Interner, Rollup, Tool};
use crate::host::normalize;
use crate::sources::parse_ts;
use rpc::{Error, Rpc};
pub use usage::Total;

/// What aitop's sessions are called in T3.
const LABEL: &str = "aitop";
const READ_SCOPES: [&str; 2] = ["diagnostics:read", "access:read"];
/// Asking the local server to pair a client needs `access:write`.
const PAIR_SCOPES: [&str; 3] = ["diagnostics:read", "access:read", "access:write"];
/// All a session on a linked machine may do.
const PEER_SCOPE: &str = "diagnostics:read";
const CALL: Duration = Duration::from_secs(30);
/// A first summary makes the server scan its transcripts.
const USAGE_CALL: Duration = Duration::from_secs(180);
/// T3 keeps about this much history.
const HISTORY_DAYS: u64 = 90;
/// Days re-read before the newest total held, which may still have been growing.
const OVERLAP_DAYS: u64 = 2;
/// How much of a machine's history has been read: bumped when a read learns
/// to keep more (costs, then other agents), so the history is read again.
const LEVEL: u8 = 2;
const TOO_OLD_HERE: &str =
    "the T3 Code on this machine is too old to share linked machines' usage; update T3 Code here";

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

/// A T3 Code server on this machine.
#[derive(Debug, Clone)]
pub struct Install {
    /// Where it answers, e.g. `http://127.0.0.1:3773`.
    origin: String,
    pub version: String,
    cli: Option<PathBuf>,
}

/// T3's data folder: `$T3CODE_HOME`, or `~/.t3`.
fn t3_home() -> PathBuf {
    std::env::var_os("T3CODE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::paths::home().join(".t3"))
}

/// The price list T3 keeps on this machine (LiteLLM's, refreshed by T3).
pub fn rates_path() -> PathBuf {
    t3_home().join("userdata/usage-model-rates.json")
}

impl Install {
    /// The server T3 last started here, going by what it wrote to its data
    /// folder. It may no longer be running; connecting finds that out.
    pub fn detect() -> Option<Install> {
        let home = t3_home();
        let text = std::fs::read_to_string(home.join("userdata/server-runtime.json")).ok()?;
        let runtime: Value = serde_json::from_str(&text).ok()?;
        let origin = runtime["origin"].as_str()?.trim_end_matches('/').to_string();
        let version = runtime["serverVersion"].as_str().unwrap_or_default().to_string();
        Some(Install { cli: find_cli(&home, &version), origin, version })
    }

    fn cli(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let cli = self.cli.as_ref().ok_or("the `t3` command was not found, so aitop cannot ask T3 Code for access")?;
        let out = Command::new(cli)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("cannot run {}: {e}", cli.display()))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let why = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no error message").trim();
            return Err(format!("`t3 {}` failed: {why}", args[..args.len().min(3)].join(" ")));
        }
        Ok(out.stdout)
    }

    /// A new session on the local server with `scopes`, valid for `ttl`.
    fn issue(&self, scopes: &[&str], ttl: &str) -> Result<Session, String> {
        let mut args = vec!["auth", "session", "issue", "--label", LABEL, "--ttl", ttl, "--json"];
        for scope in scopes {
            args.extend(["--scope", scope]);
        }
        let out = self.cli(&args)?;
        let v: Value = serde_json::from_slice(&out).map_err(|_| "`t3 auth session issue` printed no session")?;
        let token = v["token"].as_str().ok_or("`t3 auth session issue` printed no token")?;
        Ok(Session {
            id: v["sessionId"].as_str().unwrap_or_default().to_string(),
            token: token.to_string(),
            expires: v["expiresAt"].as_str().and_then(parse_ts).unwrap_or(0),
        })
    }

    fn revoke(&self, session: &Session) {
        if !session.id.is_empty() {
            let _ = self.cli(&["auth", "session", "revoke", &session.id]);
        }
    }
}

/// The `t3` command: on the PATH (the Linux package installs `t3-nightly`),
/// in the usual user folders, or next to the running server.
fn find_cli(home: &Path, version: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    let user = crate::paths::home();
    dirs.extend([user.join(".local/bin"), "/usr/local/bin".into(), "/opt/homebrew/bin".into(), "/usr/bin".into()]);
    let exe = if cfg!(windows) { ".exe" } else { "" };
    dirs.iter()
        .flat_map(|d| ["t3", "t3-nightly"].map(|name| d.join(format!("{name}{exe}"))))
        .chain([home.join("runtime/versions").join(version).join(format!("t3{exe}"))])
        .find(|p| p.is_file())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Session {
    /// T3's id for the session, for revoking it (empty when not known).
    #[serde(default)]
    id: String,
    token: String,
    /// Unix ms.
    expires: i64,
}

impl Session {
    /// Still good for a while: one about to expire is replaced early.
    fn usable(&self, now: i64) -> bool {
        self.expires - now > 10 * 60_000
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Peer {
    label: String,
    /// The address the session was made at.
    url: String,
    session: Session,
    /// Its history has been read in full at this [`LEVEL`]; later reads
    /// only go back a couple of days.
    #[serde(default)]
    level: u8,
}

/// What aitop remembers between runs, in `<data dir>/t3.json` (it holds
/// tokens, so it is only readable by the user).
#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
struct State {
    /// The server the sessions below belong to.
    origin: String,
    local: Option<Session>,
    /// Sessions on linked machines, by T3 environment id.
    peers: BTreeMap<String, Peer>,
    /// Transcript folders T3 reported for this machine, by tool key.
    roots: BTreeMap<String, Vec<PathBuf>>,
    /// This machine's other agents: the [`LEVEL`] their history was read
    /// at, and the day of the last read.
    local_level: u8,
    local_day: Option<String>,
}

pub fn state_path() -> PathBuf {
    crate::paths::data_dir().join("t3.json")
}

impl State {
    fn load(path: &Path) -> State {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
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

/// The time zone days are counted in: this machine's, so that T3's days
/// line up with aitop's ranges, or UTC when it cannot be named.
struct Zone {
    name: String,
    local: bool,
}

impl Zone {
    fn detect() -> Zone {
        match iana_time_zone::get_timezone() {
            Ok(name) if !name.trim().is_empty() => Zone { name, local: true },
            _ => Zone { name: "UTC".into(), local: false },
        }
    }

    fn today(&self) -> NaiveDate {
        if self.local {
            Local::now().date_naive()
        } else {
            Utc::now().date_naive()
        }
    }

    /// The first instant of `day` (`YYYY-MM-DD`), in unix ms.
    fn day_start(&self, day: &str) -> Option<i64> {
        let midnight = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?.and_hms_opt(0, 0, 0)?;
        if self.local {
            // Midnight can fall in a daylight-saving gap; the day then starts an hour later.
            Local
                .from_local_datetime(&midnight)
                .earliest()
                .or_else(|| Local.from_local_datetime(&(midnight + chrono::Duration::hours(1))).earliest())
                .map(|t| t.timestamp_millis())
        } else {
            Some(Utc.from_utc_datetime(&midnight).timestamp_millis())
        }
    }
}

/// One day's total and when that day starts.
#[derive(Debug, Clone, PartialEq)]
pub struct DayTotal {
    pub total: Total,
    /// Unix ms.
    pub ts: i64,
}

/// A machine linked in T3.
#[derive(Debug)]
pub struct Machine {
    /// Its T3 environment id.
    pub id: String,
    /// What T3 calls it.
    pub name: String,
    /// Its totals since the last read, or why they could not be had.
    pub result: Result<Vec<DayTotal>, String>,
}

/// What one read of T3 found.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Version of the local server.
    pub version: String,
    /// Why the local server could not be read at all.
    pub error: Option<String>,
    /// Transcript folders T3 uses on this machine.
    pub roots: Vec<(Tool, PathBuf)>,
    /// What T3 calls this machine.
    pub local_hosts: Vec<String>,
    /// This machine's usage of agents aitop has no reader for, if it has any.
    pub local: Option<Result<Vec<DayTotal>, String>>,
    pub machines: Vec<Machine>,
}

pub struct Client {
    install: Install,
    state_path: PathBuf,
    state: State,
    zone: Zone,
    /// Start of the newest day held per machine name (unix ms).
    newest: HashMap<String, i64>,
}

impl Client {
    /// A client for the T3 server on this machine, if there is one.
    pub fn detect(state_path: PathBuf) -> Option<Client> {
        let install = Install::detect()?;
        let mut state = State::load(&state_path);
        if state.origin != install.origin {
            // Sessions made at another server are no use at this one.
            state = State { origin: install.origin.clone(), roots: state.roots, ..State::default() };
        }
        Some(Client { install, state_path, state, zone: Zone::detect(), newest: HashMap::new() })
    }

    /// Version of the local server.
    pub fn version(&self) -> &str {
        &self.install.version
    }

    /// Tell the client what is already held: the newest day per machine name.
    pub fn seed(&mut self, newest: impl IntoIterator<Item = (String, i64)>) {
        self.newest.extend(newest);
    }

    /// Read T3 once.
    pub fn sync(&mut self) -> Outcome {
        let mut out = Outcome { version: self.install.version.clone(), ..Outcome::default() };
        if let Err(e) = self.read(&mut out) {
            out.error = Some(e);
        }
        // Not fatal: without the file, the next run asks for access again.
        let _ = self.state.save(&self.state_path);
        out
    }

    /// A connection to the local server, making a session for it if needed.
    fn local(&mut self) -> Result<Rpc, String> {
        let now = Local::now().timestamp_millis();
        for fresh in [false, true] {
            if fresh || !self.state.local.as_ref().is_some_and(|s| s.usable(now)) {
                self.state.local = Some(self.install.issue(&READ_SCOPES, "30d")?);
            }
            let token = &self.state.local.as_ref().expect("set above").token;
            match Rpc::connect(&self.install.origin, token) {
                Ok(rpc) => return Ok(rpc),
                // Revoked, or the server's sessions were reset: ask once more.
                Err(Error::Unauthorized) if !fresh => continue,
                Err(Error::Transport(e)) => return Err(format!("T3 Code is not answering ({e})")),
                Err(e) => return Err(format!("T3 Code refused aitop: {e}")),
            }
        }
        unreachable!("the second pass returns")
    }

    fn read(&mut self, out: &mut Outcome) -> Result<(), String> {
        let mut local = self.local()?;
        let today = self.zone.today();

        // The last scan is enough to learn which folders T3 reads here.
        let window =
            json!({ "sinceDay": day(today), "untilDay": day(today), "timeZone": self.zone.name, "provisional": true });
        let summary = local.call("server.getUsageSummary", window, USAGE_CALL).map_err(|e| e.to_string())?;
        let here = usage::parse(&summary);
        let read_here = || here.sources.iter().filter(|s| s.tool.has_reader());
        out.roots = read_here().filter(|s| s.present).map(|s| (s.tool, s.dir.clone())).collect();
        out.local_hosts = read_here().map(|s| s.host.clone()).collect();
        out.local_hosts.sort();
        out.local_hosts.dedup();
        self.state.roots = Tool::ALL
            .into_iter()
            .map(|t| {
                (t.key().to_string(), out.roots.iter().filter(|(rt, _)| *rt == t).map(|(_, p)| p.clone()).collect())
            })
            .collect();

        // Agents aitop cannot read itself are taken from T3's totals.
        if here.sources.iter().any(|s| !s.tool.has_reader() && s.present) {
            out.local = Some(self.other_agents(&mut local));
        }

        // Linked environments is a T3 setting; when it is off there are simply none.
        let links = match local.call("peerLinks.list", json!({}), CALL) {
            Ok(v) => v["links"].as_array().cloned().unwrap_or_default(),
            Err(Error::Failed { .. }) => Vec::new(),
            Err(e) => return Err(e.to_string()),
        };
        drop(local);

        let mut pairing: Option<(Rpc, Session)> = None;
        for link in &links {
            let (Some(id), Some(name)) = (link["environmentId"].as_str(), link["label"].as_str()) else { continue };
            let urls: Vec<String> =
                link["urls"].as_array().into_iter().flatten().filter_map(|u| u.as_str().map(String::from)).collect();
            let result = if link["access"].as_str() != Some("full-access") {
                Err("T3 Code only shares a machine's usage over a full-access link".to_string())
            } else {
                self.machine(id, name, &urls, &mut pairing)
            };
            out.machines.push(Machine { id: id.to_string(), name: name.to_string(), result });
        }
        // The session that may manage access has done its job.
        if let Some((rpc, session)) = pairing {
            drop(rpc);
            self.install.revoke(&session);
        }
        self.state.peers.retain(|id, _| links.iter().any(|l| l["environmentId"].as_str() == Some(id)));
        Ok(())
    }

    /// This machine's totals for the agents aitop has no reader for, since the last read.
    fn other_agents(&mut self, local: &mut Rpc) -> Result<Vec<DayTotal>, String> {
        let today = self.zone.today();
        let last_read = self
            .state
            .local_day
            .as_deref()
            .filter(|_| self.state.local_level >= LEVEL)
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
        let since = match last_read {
            Some(d) => d.min(today) - Days::new(OVERLAP_DAYS),
            None => today - Days::new(HISTORY_DAYS),
        };
        let window = json!({
            "sinceDay": day(since), "untilDay": day(today),
            "timeZone": self.zone.name, "resolution": "day", "awaitRefresh": true
        });
        let summary = local.call("server.getUsageSummary", window, USAGE_CALL).map_err(|e| e.to_string())?;
        self.state.local_level = LEVEL;
        self.state.local_day = Some(day(today));
        Ok(usage::parse(&summary)
            .totals
            .into_iter()
            .filter(|t| !t.tool.has_reader())
            .filter_map(|total| Some(DayTotal { ts: self.zone.day_start(&total.day)?, total }))
            .collect())
    }

    /// The totals of one linked machine since the newest day held.
    fn machine(
        &mut self,
        id: &str,
        name: &str,
        urls: &[String],
        pairing: &mut Option<(Rpc, Session)>,
    ) -> Result<Vec<DayTotal>, String> {
        let today = self.zone.today();
        let level = self.state.peers.get(id).map_or(0, |p| p.level);
        let caught_up = level >= LEVEL;
        let since = match self.newest.get(name).filter(|_| caught_up) {
            Some(&ms) => {
                Local.timestamp_millis_opt(ms).single().map_or(today, |t| t.date_naive()) - Days::new(OVERLAP_DAYS)
            }
            None => today - Days::new(HISTORY_DAYS),
        };
        // A day ahead, so a machine in a later time zone is not cut short.
        let window = json!({
            "sinceDay": day(since), "untilDay": day(today + Days::new(1)),
            "timeZone": self.zone.name, "resolution": "day", "awaitRefresh": true
        });

        let now = Local::now().timestamp_millis();
        for fresh in [false, true] {
            if fresh || !self.state.peers.get(id).is_some_and(|p| p.session.usable(now)) {
                let mut peer = self.pair(id, name, pairing)?;
                peer.level = level;
                self.state.peers.insert(id.to_string(), peer);
            }
            let peer = self.state.peers.get(id).expect("set above").clone();
            // The address the session was made at first, then the link's others.
            let addresses = std::iter::once(&peer.url).chain(urls.iter().filter(|u| **u != peer.url));
            let mut unreachable = String::new();
            let mut connected = None;
            for url in addresses {
                match Rpc::connect(url, &peer.session.token) {
                    Ok(rpc) => {
                        connected = Some(Ok(rpc));
                        break;
                    }
                    Err(Error::Unauthorized) => {
                        connected = Some(Err(()));
                        break;
                    }
                    Err(e) => unreachable = e.to_string(),
                }
            }
            let mut rpc = match connected {
                Some(Ok(rpc)) => rpc,
                // The session was revoked there: pair again, once.
                Some(Err(())) if !fresh => continue,
                Some(Err(())) => return Err("it does not accept the session T3 Code gave aitop".into()),
                None => return Err(format!("not reachable ({unreachable})")),
            };
            return match rpc.call("server.getUsageSummary", window.clone(), USAGE_CALL) {
                Ok(summary) => {
                    if let Some(peer) = self.state.peers.get_mut(id) {
                        peer.level = LEVEL;
                    }
                    Ok(self.totals(name, &summary))
                }
                Err(Error::Failed { tag, .. }) if tag == "EnvironmentAuthorizationError" && !fresh => continue,
                Err(e) => Err(e.to_string()),
            };
        }
        unreachable!("the second pass returns")
    }

    fn totals(&mut self, name: &str, summary: &Value) -> Vec<DayTotal> {
        let totals: Vec<DayTotal> = usage::parse(summary)
            .totals
            .into_iter()
            .filter_map(|total| Some(DayTotal { ts: self.zone.day_start(&total.day)?, total }))
            .collect();
        if let Some(newest) = totals.iter().map(|t| t.ts).max() {
            let held = self.newest.entry(name.to_string()).or_insert(newest);
            *held = (*held).max(newest);
        }
        totals
    }

    /// Get a session of aitop's own on a linked machine, through the local server.
    fn pair(&mut self, id: &str, name: &str, pairing: &mut Option<(Rpc, Session)>) -> Result<Peer, String> {
        if pairing.is_none() {
            let session = self.install.issue(&PAIR_SCOPES, "5m")?;
            let rpc = Rpc::connect(&self.install.origin, &session.token).map_err(|e| e.to_string())?;
            *pairing = Some((rpc, session));
        }
        let (rpc, _) = pairing.as_mut().expect("set above");
        let answer = match rpc.call("peerLinks.pairClient", json!({ "environmentId": id }), CALL) {
            Ok(v) => v,
            // A server from before pairing existed does not know the call.
            Err(Error::Failed { message, .. }) if message.contains("Unknown request tag") => {
                return Err(TOO_OLD_HERE.into());
            }
            Err(Error::Failed { message, .. }) => return Err(message),
            Err(e) => return Err(e.to_string()),
        };
        if answer["type"].as_str() != Some("credential") {
            return Err(match answer["reason"].as_str() {
                Some("version") => "its T3 Code is too old to share usage this way; update T3 Code there",
                Some("unreachable") => "not reachable (asleep or offline)",
                Some("link_access") => "T3 Code only shares a machine's usage over a full-access link",
                Some("refused") => "it refused; check that Linked environments is on there",
                _ => "T3 Code could not connect aitop to it",
            }
            .to_string());
        }
        let credential = answer["credential"].as_str().ok_or("T3 Code sent no pairing credential")?;
        let urls = answer["urls"].as_array().into_iter().flatten().filter_map(Value::as_str);
        let mut last = "T3 Code gave no address for it".to_string();
        for url in urls {
            match redeem(url, credential) {
                Ok(session) => return Ok(Peer { label: name.to_string(), url: url.to_string(), session, level: 0 }),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

fn day(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// Trade a pairing credential for a session at `url` that can only read usage.
fn redeem(url: &str, credential: &str) -> Result<Session, String> {
    let endpoint = format!("{}/oauth/token", url.trim_end_matches('/'));
    let form = [
        ("grant_type", "urn:ietf:params:oauth:grant-type:token-exchange"),
        ("subject_token", credential),
        ("subject_token_type", "urn:t3:params:oauth:token-type:environment-bootstrap"),
        ("requested_token_type", "urn:ietf:params:oauth:token-type:access_token"),
        ("scope", PEER_SCOPE),
        ("client_label", LABEL),
    ];
    let mut response = ureq::post(&endpoint)
        .config()
        .timeout_global(Some(CALL))
        .http_status_as_error(false)
        .build()
        .send_form(form)
        .map_err(|e| format!("not reachable ({e})"))?;
    let status = response.status();
    let body = response.body_mut().read_to_string().unwrap_or_default();
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    match v["access_token"].as_str() {
        Some(token) if status.is_success() => Ok(Session {
            id: String::new(),
            token: token.to_string(),
            expires: Local::now().timestamp_millis() + (v["expires_in"].as_f64().unwrap_or(0.0) * 1000.0) as i64,
        }),
        _ => Err(format!(
            "it would not start a session for aitop (HTTP {}{})",
            status.as_u16(),
            v["message"].as_str().or(v["error"].as_str()).map(|m| format!(": {m}")).unwrap_or_default()
        )),
    }
}

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

    #[test]
    fn days() {
        let utc = Zone { name: "UTC".into(), local: false };
        assert_eq!(utc.day_start("2026-11-14"), Some(1_794_614_400_000));
        assert_eq!(utc.day_start("soon"), None);
        let local = Zone { name: "wherever".into(), local: true };
        let start = local.day_start("2026-11-14").unwrap();
        assert_eq!(Local.timestamp_millis_opt(start).unwrap().format("%Y-%m-%d %H").to_string(), "2026-11-14 00");
        assert_eq!(day(NaiveDate::from_ymd_opt(2026, 1, 2).unwrap()), "2026-01-02");
    }

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
        // Grok has no panel of its own: it is a row, named for the agent, in the shared one.
        let grok = &events[2];
        assert_eq!((grok.tool, &*grok.project, &*grok.project_path), (Tool::Other, "Grok Build", "t3-agent://grok"));
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
