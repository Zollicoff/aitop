//! Local history: an append-only JSONL copy of every usage record.
//!
//! The tools prune their own logs (Claude Code deletes transcripts after 30
//! days by default), so without this, long-range totals would shrink over
//! time. Records are small (~200 bytes) and de-duplicated on load.

use std::borrow::Cow;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::event::{Event, Interner, Tokens, Tool};

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
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    f: bool,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

pub struct Store {
    path: PathBuf,
    writer: Option<BufWriter<File>>,
}

impl Store {
    pub fn new(path: PathBuf) -> Store {
        Store { path, writer: None }
    }

    pub fn default_path() -> PathBuf {
        crate::config::data_dir().join("history.jsonl")
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
            let Ok(r) = serde_json::from_str::<Record>(&line) else { continue };
            out.push(Event {
                tool: r.t,
                id: interner.get(&r.id),
                ts: r.ts,
                session: interner.get(&r.s),
                project: interner.get(&r.p),
                project_path: interner.get(&r.pp),
                model: interner.get(&r.m),
                tokens: Tokens {
                    input: r.i,
                    output: r.o,
                    cache_read: r.cr,
                    cache_write: r.cw,
                    cache_write_1h: r.cw1,
                    reasoning: r.r,
                },
                fast: r.f,
                cost: 0.0,
                priced: false,
            });
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
            let r = Record {
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
                f: e.fast,
            };
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
            id: i.get("x:1"),
            ts: 42,
            session: i.get("s"),
            project: i.get("p"),
            project_path: i.get("/p"),
            model: i.get("gpt-5"),
            tokens: Tokens { input: 1, output: 2, cache_read: 3, cache_write: 0, cache_write_1h: 0, reasoning: 1 },
            fast: false,
            cost: 0.0,
            priced: false,
        };
        let mut s = Store::new(dir.path().join("sub/h.jsonl"));
        s.append(&[&e, &e]).unwrap();
        std::fs::OpenOptions::new().append(true).open(s.path()).unwrap().write_all(b"garbage\n").unwrap();
        let back = s.load(&mut i).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].tokens, e.tokens);
        assert_eq!(&*back[0].model, "gpt-5");
    }
}
