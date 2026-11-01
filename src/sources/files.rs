//! Finding log files.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// All files under `roots` (recursively) for which `keep` is true, sorted by
/// file name. Both tools put a timestamp or UUID in the name, so for Codex
/// this order is chronological.
pub fn discover(roots: &[PathBuf], keep: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        walk(root, &keep, &mut out);
    }
    out.sort_by(|a, b| a.file_name().cmp(&b.file_name()).then_with(|| a.cmp(b)));
    out
}

fn walk(dir: &Path, keep: &impl Fn(&Path) -> bool, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
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

/// Forget per-file state for files that no longer exist.
pub fn prune<T>(state: &mut HashMap<PathBuf, T>, present: &[PathBuf]) {
    let present: HashSet<&PathBuf> = present.iter().collect();
    state.retain(|p, _| present.contains(p));
}

/// RFC 3339 timestamp to unix milliseconds.
pub fn parse_ts(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_and_prunes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        for f in ["a/b/2.jsonl", "a/1.jsonl", "a/skip.txt"] {
            std::fs::write(dir.path().join(f), "").unwrap();
        }
        let found = discover(&[dir.path().to_path_buf(), dir.path().join("missing")], |p| {
            p.extension().is_some_and(|e| e == "jsonl")
        });
        let names: Vec<_> = found.iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, ["1.jsonl", "2.jsonl"]);
        let mut state: HashMap<PathBuf, ()> = HashMap::from([(found[0].clone(), ()), (dir.path().join("gone"), ())]);
        prune(&mut state, &found);
        assert_eq!(state.len(), 1);
    }

    #[test]
    fn parses_timestamps() {
        assert_eq!(parse_ts("1970-01-01T00:00:01.5Z"), Some(1500));
        assert_eq!(parse_ts("nope"), None);
    }
}
