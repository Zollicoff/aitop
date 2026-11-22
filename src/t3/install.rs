//! Finding the T3 Code installed on this machine, and asking it for access
//! through its own command-line tool.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use super::state::Session;
use crate::sources::parse_ts;

/// What aitop's sessions are called in T3.
pub(super) const LABEL: &str = "aitop";

/// A T3 Code server on this machine.
#[derive(Debug, Clone)]
pub struct Install {
    /// Where it answers, e.g. `http://127.0.0.1:3773`.
    pub(super) origin: String,
    pub version: String,
    pub(super) cli: Option<PathBuf>,
}

/// T3's data folder: `$T3CODE_HOME`, or `~/.t3`.
pub(super) fn t3_home() -> PathBuf {
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
    pub(super) fn issue(&self, scopes: &[&str], ttl: &str) -> Result<Session, String> {
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

    pub(super) fn revoke(&self, session: &Session) {
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
