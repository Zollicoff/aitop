//! Compressed logs: Codex packs older sessions into `rollout-*.jsonl.zst`.
//!
//! Such a log is complete, so it is read whole. The records found in it are
//! kept in the cache directory, keyed by the log's path, size and time, so
//! that each log is only decompressed once.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::billing::FileState;
use super::CodexSource;
use crate::event::Event;
use crate::sources::Ctx;

/// Bumped when records are made differently, so cached ones are made again.
const ARCHIVE_FORMAT: u32 = 1;

/// The first line of a cached archive: which log its records were read from.
#[derive(Serialize, Deserialize, PartialEq)]
struct ArchiveKey {
    format: u32,
    path: String,
    len: u64,
    mtime: i64,
}

pub(super) fn is_compressed(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.to_string_lossy().ends_with(".jsonl.zst"))
}

/// A stable name for `path`'s cache file (FNV-1a).
fn cache_name(path: &Path) -> String {
    let hash = path
        .to_string_lossy()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("{hash:016x}.jsonl")
}

impl CodexSource {
    /// Add the records of a compressed log to `out`: from the cache if it
    /// has been read before, otherwise by decompressing it.
    pub(super) fn read_compressed(&mut self, path: &Path, ctx: &mut Ctx, out: &mut Vec<Event>) -> std::io::Result<()> {
        use std::io::{BufRead, BufReader, Write};
        let meta = std::fs::metadata(path)?;
        let key = ArchiveKey {
            format: ARCHIVE_FORMAT,
            path: path.to_string_lossy().to_string(),
            len: meta.len(),
            mtime: crate::sources::files::modified(path).unwrap_or(0),
        };
        let cached = self.archive_cache.as_ref().map(|dir| dir.join(cache_name(path)));
        if let Some(text) = cached.as_ref().and_then(|c| std::fs::read_to_string(c).ok()) {
            let mut lines = text.lines();
            if lines.next().and_then(|l| serde_json::from_str::<ArchiveKey>(l).ok()).as_ref() == Some(&key) {
                out.extend(lines.filter_map(|l| crate::store::decode(l, ctx.interner, ctx.host)).map(|mut e| {
                    e.host = ctx.host.clone();
                    e
                }));
                return Ok(());
            }
        }

        let stem = path.file_name().unwrap_or_default().to_string_lossy();
        let fallback_session = ctx.interner.get(stem.trim_end_matches(".jsonl.zst"));
        let mut st = FileState::default();
        let mut events = Vec::new();
        let mut reader =
            BufReader::with_capacity(256 * 1024, zstd::stream::read::Decoder::new(std::fs::File::open(path)?)?);
        let mut buf = Vec::with_capacity(16 * 1024);
        loop {
            buf.clear();
            if reader.read_until(b'\n', &mut buf)? == 0 {
                break;
            }
            let line = buf.trim_ascii_end();
            if !line.is_empty() {
                self.handle_line(line, &mut st, &fallback_session, ctx, &mut events);
            }
        }
        // Best effort: without the cache the log is decompressed again next time.
        if let Some(cached) = cached {
            let mut text = serde_json::to_string(&key).unwrap_or_default();
            for e in &events {
                text.push('\n');
                text.push_str(&crate::store::encode(e));
            }
            text.push('\n');
            if let Some(dir) = cached.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = cached.with_extension("tmp");
            let written = std::fs::File::create(&tmp).and_then(|mut f| f.write_all(text.as_bytes()));
            let _ = written.and_then(|()| std::fs::rename(&tmp, &cached));
        }
        out.extend(events);
        Ok(())
    }
}
