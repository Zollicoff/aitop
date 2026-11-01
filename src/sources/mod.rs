//! Log readers for each tool. All readers are incremental: a file is only
//! re-read from where the previous scan stopped, so polling every couple of
//! seconds stays cheap even with gigabytes of history.

pub mod claude;
pub mod codex;
pub mod gemini;

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::event::{Event, Interner, Tool};
use crate::project::ProjectResolver;

pub struct Ctx<'a> {
    pub interner: &'a mut Interner,
    pub projects: &'a mut ProjectResolver,
}

#[derive(Clone, Debug)]
pub struct SourceStatus {
    pub tool: Tool,
    /// Root directories and whether they exist.
    pub roots: Vec<(PathBuf, bool)>,
    pub files: usize,
    pub errors: Vec<String>,
}

impl SourceStatus {
    pub fn new(tool: Tool, roots: &[PathBuf]) -> Self {
        SourceStatus {
            tool,
            roots: roots.iter().map(|r| (r.clone(), r.is_dir())).collect(),
            files: 0,
            errors: Vec::new(),
        }
    }

    pub fn detected(&self) -> bool {
        self.roots.iter().any(|(_, e)| *e)
    }
}

pub trait Source: Send {
    fn scan(&mut self, ctx: &mut Ctx, out: &mut Vec<Event>) -> SourceStatus;
}

/// Position of the incremental reader within one file.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tail {
    pub offset: u64,
    pub len: u64,
    pub mtime: i64,
}

pub enum Change {
    Unchanged,
    /// New data was appended.
    Appended,
    /// The file shrank or was replaced; state derived from it must be reset.
    Reset,
}

pub fn file_stamp(path: &Path) -> Option<(u64, i64)> {
    let m = std::fs::metadata(path).ok()?;
    let mtime = m.modified().ok()?.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    Some((m.len(), mtime))
}

/// Call `f` for every complete line appended since the last call. A trailing
/// line without a newline is left for the next scan (it may still be being
/// written).
pub fn read_new_lines(path: &Path, tail: &mut Tail, mut f: impl FnMut(&[u8])) -> std::io::Result<Change> {
    let Some((len, mtime)) = file_stamp(path) else {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "vanished"));
    };
    if len == tail.len && mtime == tail.mtime {
        return Ok(Change::Unchanged);
    }
    let mut change = Change::Appended;
    if len < tail.offset {
        *tail = Tail::default();
        change = Change::Reset;
    }
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(tail.offset))?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut buf = Vec::with_capacity(16 * 1024);
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 || buf.last() != Some(&b'\n') {
            break;
        }
        tail.offset += n as u64;
        let line = trim_line(&buf);
        if !line.is_empty() {
            f(line);
        }
    }
    tail.len = len;
    tail.mtime = mtime;
    Ok(change)
}

fn trim_line(b: &[u8]) -> &[u8] {
    let mut end = b.len();
    while end > 0 && matches!(b[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    &b[..end]
}

/// Recursively collect files under `root` whose name satisfies `keep`.
pub fn walk(root: &Path, keep: &dyn Fn(&Path) -> bool, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let p = entry.path();
        if ft.is_dir() {
            walk(&p, keep, out);
        } else if ft.is_file() && keep(&p) {
            out.push(p);
        }
    }
}

pub fn has_ext(p: &Path, ext: &str) -> bool {
    p.extension().is_some_and(|e| e == ext)
}

pub fn parse_ts(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn incremental_reads() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        std::fs::write(&p, "one\ntwo\npart").unwrap();
        let mut t = Tail::default();
        let mut got = Vec::new();
        read_new_lines(&p, &mut t, |l| got.push(String::from_utf8_lossy(l).to_string())).unwrap();
        assert_eq!(got, ["one", "two"]);
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        writeln!(f, "ial").unwrap();
        writeln!(f, "three").unwrap();
        got.clear();
        read_new_lines(&p, &mut t, |l| got.push(String::from_utf8_lossy(l).to_string())).unwrap();
        assert_eq!(got, ["partial", "three"]);
        got.clear();
        assert!(matches!(read_new_lines(&p, &mut t, |_| {}).unwrap(), Change::Unchanged));
        std::fs::write(&p, "x\n").unwrap();
        assert!(matches!(
            read_new_lines(&p, &mut t, |l| got.push(String::from_utf8_lossy(l).to_string())).unwrap(),
            Change::Reset
        ));
        assert_eq!(got, ["x"]);
    }
}
