//! Reading Codex logs, end to end.

use std::sync::Arc;

use serde_json::Value;

use super::billing::{is_fork, speed};
use super::*;
use crate::event::Interner;
use crate::event::{Speed, Tokens};
use crate::project::ProjectResolver;

const META: &str = r#"{"timestamp":"2026-11-01T10:00:00Z","type":"session_meta","payload":{"id":"sess-1","cwd":"/nowhere/webapp","cli_version":"0.50.0"}}"#;
const TURN: &str = r#"{"timestamp":"2026-11-01T10:00:01Z","type":"turn_context","payload":{"cwd":"/nowhere/webapp","model":"gpt-5-codex"}}"#;

/// A `token_count` line: this request's `(input, cached, output, reasoning)` and the running total's.
fn tc(ts: &str, last: (u64, u64, u64, u64), total: (u64, u64, u64, u64), limits: &str) -> String {
    let usage = |(input, cached, output, reasoning): (u64, u64, u64, u64)| {
        format!(
            r#"{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output},"reasoning_output_tokens":{reasoning},"total_tokens":{}}}"#,
            input + output
        )
    };
    format!(
        r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{},"last_token_usage":{},"model_context_window":272000}},"rate_limits":{limits}}}}}"#,
        usage(total),
        usage(last)
    )
}

/// Scan `files` (name, lines) from a fresh source.
fn scan(files: &[(&str, Vec<String>)]) -> (Vec<Event>, CodexSource, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let day = dir.path().join("2026/11/01");
    std::fs::create_dir_all(&day).unwrap();
    for (name, lines) in files {
        std::fs::write(day.join(name), lines.join("\n") + "\n").unwrap();
    }
    let mut src = CodexSource::new(vec![dir.path().to_path_buf()]);
    src.archive_cache = Some(dir.path().join("cache"));
    let mut interner = Interner::default();
    let mut projects = ProjectResolver::new();
    let host: Arc<str> = "here".into();
    let mut ctx = Ctx { interner: &mut interner, projects: &mut projects, host: &host, since: None };
    let mut out = Vec::new();
    src.scan(&mut ctx, &mut out);
    (out, src, dir)
}

#[test]
fn one_record_per_request() {
    let limits = r#"{"primary":{"used_percent":34.0,"window_minutes":300,"resets_in_seconds":600},"secondary":{"used_percent":12.5,"window_minutes":10080,"resets_at":1800000000}}"#;
    let lines = vec![
        META.to_string(),
        TURN.to_string(),
        r#"{"timestamp":"2026-11-01T10:00:02Z","type":"response_item","payload":{"type":"message","content":[{"type":"output_text","text":"token_count"}]}}"#.to_string(),
        tc("2026-11-01T10:00:05Z", (1000, 200, 100, 40), (1000, 200, 100, 40), "null"),
        tc("2026-11-01T10:00:06Z", (1000, 200, 100, 40), (1000, 200, 100, 40), "null"), // the same snapshot again
        tc("2026-11-01T10:01:00Z", (2000, 1000, 200, 50), (3000, 1200, 300, 90), limits),
        // A context-size marker, and a total that does not follow from the request: neither is one.
        tc("2026-11-01T10:01:30Z", (0, 0, 0, 0), (3000, 1200, 300, 90), "null"),
        tc("2026-11-01T10:02:00Z", (500, 0, 50, 0), (9000, 1200, 900, 90), "null"),
    ];
    let (out, src, _dir) = scan(&[("rollout-2026-11-01T10-00-00-sess-1.jsonl", lines)]);
    assert_eq!(out.len(), 2);
    assert_eq!(
        out[0].tokens,
        Tokens { input: 800, output: 100, cache_read: 200, cache_write: 0, cache_write_1h: 0, reasoning: 40 }
    );
    assert_eq!(
        out[1].tokens,
        Tokens { input: 1000, output: 200, cache_read: 1000, cache_write: 0, cache_write_1h: 0, reasoning: 50 }
    );
    assert_eq!((&*out[0].model, &*out[0].project, &*out[0].session), ("gpt-5-codex", "webapp", "sess-1"));
    assert_eq!(out[0].speed, Speed::Standard);
    assert_ne!(out[0].id, out[1].id);
    let l = src.rate_limits().cloned().unwrap();
    assert_eq!(l.primary.as_ref().unwrap().used_percent, 34.0);
    assert_eq!(l.primary.as_ref().unwrap().resets_at, Some(out[1].ts + 600_000));
    assert_eq!(l.secondary.as_ref().unwrap().resets_at, Some(1_800_000_000_000));
}

#[test]
fn a_fork_does_not_bill_its_parents_history_again() {
    let meta = r#"{"timestamp":"2026-11-01T12:00:00.000Z","type":"session_meta","payload":{"id":"child","cwd":"/nowhere/webapp","forked_from_id":"sess-1"}}"#;
    let parent_meta = r#"{"timestamp":"2026-11-01T12:00:00.010Z","type":"session_meta","payload":{"id":"sess-1","cwd":"/elsewhere"}}"#;
    let lines = vec![
        meta.to_string(),
        parent_meta.to_string(),
        r#"{"timestamp":"2026-11-01T12:00:00.020Z","type":"turn_context","payload":{"model":"gpt-5-codex"}}"#
            .to_string(),
        // The parent's two requests, copied in within milliseconds.
        tc("2026-11-01T12:00:00.030Z", (1000, 200, 100, 40), (1000, 200, 100, 40), "null"),
        tc("2026-11-01T12:00:00.050Z", (2000, 1000, 200, 50), (3000, 1200, 300, 90), "null"),
        // The child's own request, seconds later, continuing from the copied total.
        tc("2026-11-01T12:00:09Z", (4000, 3000, 80, 0), (7000, 4200, 380, 90), "null"),
    ];
    let (out, _, _dir) = scan(&[("rollout-2026-11-01T12-00-00-child.jsonl", lines)]);
    assert_eq!(out.len(), 1, "only the child's own request");
    assert_eq!((out[0].tokens.input, out[0].tokens.cache_read, out[0].tokens.output), (1000, 3000, 80));
    assert_eq!((&*out[0].session, &*out[0].project), ("child", "webapp"), "the first meta names the session");

    // A sub-agent is a fork too.
    let sub = r#"{"timestamp":"2026-11-01T12:00:00.000Z","type":"session_meta","payload":{"id":"sub","source":{"subagent":{"thread_spawn":{"parent_thread_id":"sess-1"}}}}}"#;
    assert!(is_fork(&serde_json::from_str::<Value>(sub).unwrap()["payload"]));
    assert!(!is_fork(&serde_json::from_str::<Value>(META).unwrap()["payload"]));
}

#[test]
fn resumed_and_copied_logs_count_once() {
    let first = vec![
        META.to_string(),
        TURN.to_string(),
        tc("2026-11-01T10:00:05Z", (1000, 200, 100, 40), (1000, 200, 100, 40), "null"),
    ];
    // Resumed in a new file: the total carries on, and only the new request is billed.
    let resumed = vec![
        META.replace("10:00:00", "12:00:00"),
        TURN.replace("10:00:01", "12:00:01"),
        tc("2026-11-01T12:00:05Z", (500, 0, 50, 0), (1500, 200, 150, 40), "null"),
        // A new process starts the total again at its first request.
        tc("2026-11-01T12:10:00Z", (700, 0, 70, 0), (700, 0, 70, 0), "null"),
        tc("2026-11-01T12:10:00Z", (700, 0, 70, 0), (700, 0, 70, 0), "null"),
    ];
    let (out, _, _dir) = scan(&[
        ("rollout-2026-11-01T10-00-00-sess-1.jsonl", first.clone()),
        ("rollout-2026-11-01T10-00-00-sess-1-copy.jsonl", first),
        ("rollout-2026-11-01T12-00-00-sess-1.jsonl", resumed),
    ]);
    let mut ids: Vec<&str> = out.iter().map(|e| &*e.id).collect();
    assert_eq!(ids.len(), 4);
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3, "the copied log repeats an id, which the collector drops");
    let outputs: Vec<u64> = out.iter().map(|e| e.tokens.output).collect();
    assert_eq!(outputs, [100, 100, 50, 70]);
}

#[test]
fn speed_tier_and_listed_requests() {
    let settings = |ts: &str, tier: &str| {
        format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"thread_settings_applied","thread_settings":{{"service_tier":{tier}}}}}}}"#
        )
    };
    let record = |ts: &str, input: u64, output: u64, response: &str| {
        format!(
            r#"{{"timestamp":"{ts}","type":"token_usage_record","payload":{{"usage":{{"input_tokens":{input},"output_tokens":{output}}}{response}}}}}"#
        )
    };
    let lines = vec![
        META.to_string(),
        TURN.to_string(),
        settings("2026-11-01T10:00:02Z", r#""priority""#),
        tc("2026-11-01T10:00:05Z", (1000, 200, 100, 40), (1000, 200, 100, 40), "null"),
        settings("2026-11-01T10:00:06Z", "null"),
        // Two requests listed without an id; the snapshot's total accounts for both.
        record("2026-11-01T10:00:10Z", 300, 30, ""),
        record("2026-11-01T10:00:20Z", 400, 40, ""),
        tc("2026-11-01T10:00:21Z", (400, 0, 40, 0), (1700, 200, 170, 40), "null"),
        // Identified requests are billed as they come; snapshots then only move the total.
        record("2026-11-01T10:01:00Z", 600, 60, r#","response_id":"resp_1""#),
        tc("2026-11-01T10:01:01Z", (600, 0, 60, 0), (2300, 200, 230, 40), "null"),
        record("2026-11-01T10:01:00Z", 600, 60, r#","response_id":"resp_1""#),
    ];
    let (out, _, _dir) = scan(&[("rollout-2026-11-01T10-00-00-sess-1.jsonl", lines)]);
    let seen: Vec<(u64, Speed)> = out.iter().map(|e| (e.tokens.output, e.speed)).collect();
    assert_eq!(
        seen,
        [
            (100, Speed::Fast),
            (30, Speed::Standard),
            (40, Speed::Standard),
            (60, Speed::Standard),
            (60, Speed::Standard)
        ]
    );
    assert_eq!(out[3].id, out[4].id, "a response id is its own identity");
    assert!(out[3].id.ends_with(":r:resp_1"));
    assert_eq!(speed(&Value::from("fast")), Speed::Fast);
    assert_eq!(speed(&Value::from("ultrafast")), Speed::Ultrafast);
    assert_eq!(speed(&Value::from("default")), Speed::Standard);
}

#[test]
fn compressed_logs_are_read_once() {
    let lines = [
        META.to_string(),
        TURN.to_string(),
        tc("2026-11-01T10:00:05Z", (1000, 200, 100, 40), (1000, 200, 100, 40), "null"),
        tc("2026-11-01T10:01:00Z", (2000, 1000, 200, 50), (3000, 1200, 300, 90), "null"),
    ];
    let plain = lines.join("\n") + "\n";
    let (expect, _, _d) = scan(&[("rollout-2026-11-01T10-00-00-sess-1.jsonl", lines.to_vec())]);

    // The same log as Codex leaves it once it has compressed it.
    let (none, mut src, dir) = scan(&[]);
    assert!(none.is_empty());
    let log = dir.path().join("2026/11/01/rollout-2026-11-01T10-00-00-sess-1.jsonl.zst");
    std::fs::write(&log, zstd::encode_all(plain.as_bytes(), 3).unwrap()).unwrap();
    let mut interner = Interner::default();
    let mut projects = ProjectResolver::new();
    let host: Arc<str> = "here".into();
    let mut ctx = Ctx { interner: &mut interner, projects: &mut projects, host: &host, since: None };
    let mut out = Vec::new();
    let status = src.scan(&mut ctx, &mut out);
    assert_eq!((status.files, status.errors.len()), (1, 0));
    let same = |a: &[Event], b: &[Event]| {
        a.len() == b.len()
            && a.iter().zip(b).all(|(x, y)| (&x.id, x.ts, x.tokens, &x.model) == (&y.id, y.ts, y.tokens, &y.model))
    };
    assert!(same(&out, &expect), "the same records, with the same ids, as from the plain log");
    // Nothing new on a rescan.
    out.clear();
    src.scan(&mut ctx, &mut out);
    assert!(out.is_empty());

    // A new process gets the records from the cache without decompressing:
    // it still does when the log itself is no longer a valid archive of that size.
    let cache: Vec<_> = std::fs::read_dir(dir.path().join("cache")).unwrap().flatten().collect();
    assert_eq!(cache.len(), 1);
    let len = std::fs::metadata(&log).unwrap().len() as usize;
    let mtime = std::fs::metadata(&log).unwrap().modified().unwrap();
    std::fs::write(&log, vec![0u8; len]).unwrap();
    std::fs::File::options().write(true).open(&log).unwrap().set_modified(mtime).unwrap();
    let mut again = CodexSource::new(vec![dir.path().to_path_buf()]);
    again.archive_cache = Some(dir.path().join("cache"));
    again.scan(&mut ctx, &mut out);
    assert!(same(&out, &expect));
    assert_eq!((&*out[0].project, &*out[0].host), ("webapp", "here"));
    // Without the cache, the damaged log is an error, not a crash.
    again = CodexSource::new(vec![dir.path().to_path_buf()]);
    again.archive_cache = None;
    out.clear();
    assert_eq!(again.scan(&mut ctx, &mut out).errors.len(), 1);
    assert!(out.is_empty());
}
