//! Reading T3 Code: the local server for its folders and other agents, and
//! each linked machine for its totals.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{Days, Local, NaiveDate, TimeZone, Utc};
use serde_json::{json, Value};

use super::install::{Install, LABEL};
use super::rpc::{Error, Rpc};
use super::state::{Peer, Session, State, LEVEL};
use super::usage::{self, Total};
use crate::event::Tool;

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
const TOO_OLD_HERE: &str =
    "the T3 Code on this machine is too old to share linked machines' usage; update T3 Code here";

/// The time zone days are counted in: this machine's, so that T3's days
/// line up with aitop's ranges, or UTC when it cannot be named.
pub(super) struct Zone {
    pub(super) name: String,
    pub(super) local: bool,
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
    pub(super) fn day_start(&self, day: &str) -> Option<i64> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
