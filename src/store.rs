//! Local history: an append-only JSONL copy of every usage record.
//!
//! The tools prune their own logs (Claude Code deletes transcripts after 30
//! days by default), so without this, long-range totals would shrink over
//! time. Records are small (~200 bytes) and de-duplicated on load.
//!
//! The same record format is what `aitop export` prints and what another
//! machine's aitop reads when it syncs (see [`crate::remote`]). A record
//! names its machine in `h`; in the history file that is left out for this
//! machine's own usage, so the file stays valid if the machine is renamed.

use std::borrow::Cow;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::event::{Event, Interner, Rollup, Speed, Tokens, Tool};

#[derive(Serialize, Deserialize)]
struct Record<'a> {
    t: Tool,
    #[serde(borrow)]
    id: Cow<'a, str>,
    ts: i64,
    #[serde(borrow)]
    s: Cow<'a, str>,
    #[serde(borrow)]
    p: Cow<'a, str>,
    #[serde(borrow)]
    pp: Cow<'a, str>,
    #[serde(borrow)]
    m: Cow<'a, str>,
    #[serde(default, skip_serializing_if = "is_zero")]
    i: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    o: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    cr: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    cw: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    cw1: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    r: u64,
    /// Fast and ultrafast speed tiers.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    f: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    uf: bool,
    /// Machine, when it is not the one the file belongs to.
    #[serde(borrow, default, skip_serializing_if = "Option::is_none")]
    h: Option<Cow<'a, str>>,
    /// Responses and sessions, when the record is a total rather than one response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ns: Option<u32>,
    /// A total's cost as reported with it, and whether that leaves some out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    c: Option<f64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    u: bool,
}

impl<'a> Record<'a> {
    fn of(e: &'a Event, host: Option<&'a str>) -> Record<'a> {
        Record {
            t: e.tool,
            id: Cow::Borrowed(&e.id),
            ts: e.ts,
            s: Cow::Borrowed(&e.session),
            p: Cow::Borrowed(&e.project),
            pp: Cow::Borrowed(&e.project_path),
            m: Cow::Borrowed(&e.model),
            i: e.tokens.input,
            o: e.tokens.output,
            cr: e.tokens.cache_read,
            cw: e.tokens.cache_write,
            cw1: e.tokens.cache_write_1h,
            r: e.tokens.reasoning,
            f: e.speed == Speed::Fast,
            uf: e.speed == Speed::Ultrafast,
            h: host.map(Cow::Borrowed),
            n: e.rollup.map(|r| r.responses),
            ns: e.rollup.map(|r| r.sessions),
            c: e.rollup.and_then(|r| r.cost),
            u: e.rollup.is_some_and(|r| r.unpriced),
        }
    }

    fn into_event(self, interner: &mut Interner, default_host: &Arc<str>) -> Event {
        Event {
            tool: self.t,
            host: match &self.h {
                Some(h) => interner.get(h),
                None => default_host.clone(),
            },
            id: interner.get(&self.id),
            ts: self.ts,
            session: interner.get(&self.s),
            project: interner.get(&self.p),
            project_path: interner.get(&self.pp),
            model: interner.get(&self.m),
            tokens: Tokens {
                input: self.i,
                output: self.o,
                cache_read: self.cr,
                cache_write: self.cw,
                cache_write_1h: self.cw1,
                reasoning: self.r,
            },
            speed: match (self.uf, self.f) {
                (true, _) => Speed::Ultrafast,
                (false, true) => Speed::Fast,
                (false, false) => Speed::Standard,
            },
            cost: 0.0,
            priced: false,
            rollup: self.n.map(|responses| Rollup {
                responses,
                sessions: self.ns.unwrap_or(0),
                cost: self.c,
                unpriced: self.u,
            }),
        }
    }
}

/// One record as a line of JSON that names its machine.
pub fn encode(e: &Event) -> String {
    serde_json::to_string(&Record::of(e, Some(&e.host))).expect("records serialize")
}

/// Parse a line written by [`encode`] or found in a history file. A record
/// that names no machine belongs to `default_host`.
pub fn decode(line: &str, interner: &mut Interner, default_host: &Arc<str>) -> Option<Event> {
    let r: Record = serde_json::from_str(line).ok()?;
    Some(r.into_event(interner, default_host))
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

pub struct Store {
    path: PathBuf,
    /// The machine this history belongs to.
    host: Arc<str>,
    writer: Option<BufWriter<File>>,
}

impl Store {
    pub fn new(path: PathBuf, host: Arc<str>) -> Store {
        Store { path, host, writer: None }
    }

    pub fn default_path() -> PathBuf {
        crate::paths::data_dir().join("history.jsonl")
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read all stored records. Unreadable lines are skipped.
    pub fn load(&self, interner: &mut Interner) -> Result<Vec<Event>> {
        let f = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", self.path.display())),
        };
        let mut out = Vec::new();
        for line in BufReader::new(f).lines() {
            let Ok(line) = line else { break };
            if let Some(e) = decode(&line, interner, &self.host) {
                out.push(e);
            }
        }
        Ok(out)
    }

    pub fn append(&mut self, events: &[&Event]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        if self.writer.is_none() {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let f = OpenOptions::new().create(true).append(true).open(&self.path)?;
            self.writer = Some(BufWriter::new(f));
        }
        let w = self.writer.as_mut().expect("opened above");
        for e in events {
            let host = (e.host != self.host).then_some(&*e.host);
            let r = Record::of(e, host);
            serde_json::to_writer(&mut *w, &r)?;
            w.write_all(b"\n")?;
        }
        w.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut i = Interner::default();
        let e = Event {
            tool: Tool::Codex,
            host: i.get("here"),
            id: i.get("x:1"),
            ts: 42,
            session: i.get("s"),
            project: i.get("p"),
            project_path: i.get("/p"),
            model: i.get("gpt-5"),
            tokens: Tokens { input: 1, output: 2, cache_read: 3, cache_write: 0, cache_write_1h: 0, reasoning: 1 },
            speed: Speed::Ultrafast,
            cost: 0.0,
            priced: false,
            rollup: None,
        };
        let other = Event { host: i.get("laptop"), id: i.get("x:2"), ..e.clone() };
        let mut s = Store::new(dir.path().join("sub/h.jsonl"), i.get("here"));
        s.append(&[&e, &other]).unwrap();
        std::fs::OpenOptions::new().append(true).open(s.path()).unwrap().write_all(b"garbage\n").unwrap();
        let back = s.load(&mut i).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].tokens, e.tokens);
        assert_eq!(&*back[0].model, "gpt-5");
        assert_eq!(back[0].speed, Speed::Ultrafast);
        // Only another machine's records name their machine in the file...
        let text = std::fs::read_to_string(s.path()).unwrap();
        assert_eq!(text.matches("\"h\"").count(), 1, "{text}");
        assert_eq!((&*back[0].host, &*back[1].host), ("here", "laptop"));
        // ...but an exported record always does.
        let sent = decode(&encode(&e), &mut i, &Arc::from("elsewhere")).unwrap();
        assert_eq!(&*sent.host, "here");
        assert_eq!(sent.tokens, e.tokens);
        // A total keeps what it stands for.
        let rollup = Rollup { responses: 40, sessions: 3, cost: Some(1.25), unpriced: true };
        let total = Event { rollup: Some(rollup), ..e.clone() };
        let back = decode(&encode(&total), &mut i, &Arc::from("x")).unwrap();
        assert_eq!(back.rollup, total.rollup);
        assert_eq!((back.responses(), e.responses()), (40, 1));
    }
}
