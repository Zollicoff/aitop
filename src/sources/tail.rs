//! Incremental reading of append-only log files.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Position of the incremental reader within one file.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tail {
    offset: u64,
    len: u64,
    mtime: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    Unchanged,
    /// New data may have been appended.
    Grown,
    /// The file shrank, so it was rewritten: reading restarts at the top and
    /// anything derived from the old contents should be discarded.
    Reset,
}

impl Tail {
    /// Check whether the file changed since the last read.
    pub fn poll(&mut self, path: &Path) -> std::io::Result<Change> {
        let m = std::fs::metadata(path)?;
        let mtime = m.modified()?.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        let len = m.len();
        if len == self.len && mtime == self.mtime {
            return Ok(Change::Unchanged);
        }
        let reset = len < self.offset;
        if reset {
            self.offset = 0;
        }
        self.len = len;
        self.mtime = mtime;
        Ok(if reset { Change::Reset } else { Change::Grown })
    }

    /// Call `f` for every complete line from the current position. A final
    /// line without a newline is left for later (it may still be being written).
    pub fn read_lines(&mut self, path: &Path, mut f: impl FnMut(&[u8])) -> std::io::Result<()> {
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut reader = BufReader::with_capacity(256 * 1024, file);
        let mut buf = Vec::with_capacity(16 * 1024);
        loop {
            buf.clear();
            let n = reader.read_until(b'\n', &mut buf)?;
            if n == 0 || buf.last() != Some(&b'\n') {
                return Ok(());
            }
            self.offset += n as u64;
            let line = buf.trim_ascii_end();
            if !line.is_empty() {
                f(line);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn read(t: &mut Tail, p: &Path) -> (Change, Vec<String>) {
        let c = t.poll(p).unwrap();
        let mut got = Vec::new();
        if c != Change::Unchanged {
            t.read_lines(p, |l| got.push(String::from_utf8_lossy(l).to_string())).unwrap();
        }
        (c, got)
    }

    #[test]
    fn incremental_reads() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        std::fs::write(&p, "one\ntwo\npart").unwrap();
        let mut t = Tail::default();
        assert_eq!(read(&mut t, &p), (Change::Grown, vec!["one".into(), "two".into()]));
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        writeln!(f, "ial\r").unwrap();
        writeln!(f, "three").unwrap();
        assert_eq!(read(&mut t, &p), (Change::Grown, vec!["partial".into(), "three".into()]));
        assert_eq!(read(&mut t, &p).0, Change::Unchanged);
        std::fs::write(&p, "x\n").unwrap();
        assert_eq!(read(&mut t, &p), (Change::Reset, vec!["x".into()]));
    }
}
