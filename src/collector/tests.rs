//! The collector end to end: merging, history, other machines and T3 Code.

use std::sync::atomic::Ordering;

use super::*;

#[test]
fn dedupe_keeps_largest_output_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("claude/-x-proj");
    std::fs::create_dir_all(&logs).unwrap();
    let mk = |out: u64| {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-11-01T10:00:00Z","sessionId":"s","requestId":"r","message":{{"id":"m","model":"claude-sonnet-4-5","usage":{{"input_tokens":100000,"output_tokens":{out}}}}}}}"#
        )
    };
    std::fs::write(logs.join("s.jsonl"), format!("{}\n{}\n", mk(10), mk(1_000_000))).unwrap();
    let cfg = Config {
        paths: crate::config::PathsConfig {
            claude: vec![dir.path().join("claude")],
            codex: vec![dir.path().join("none")],
        },
        host: Some("desk".into()),
        ..Default::default()
    };
    let hist = dir.path().join("history.jsonl");
    let mut c = Collector::new(&cfg, Some(hist.clone()));
    c.load_history();
    c.scan();
    {
        let d = c.data.read().unwrap();
        assert_eq!(d.events.len(), 1);
        assert_eq!(d.events[0].tokens.output, 1_000_000);
        assert!((d.events[0].cost - 15.3).abs() < 1e-9, "sonnet 4.5: $0.30 in + $15 out");
    }
    // Logs deleted: history still has the record.
    std::fs::remove_dir_all(dir.path().join("claude")).unwrap();
    let mut c2 = Collector::new(&cfg, Some(hist));
    c2.load_history();
    c2.scan();
    let d = c2.data.read().unwrap();
    assert_eq!(d.events.len(), 1);
    assert_eq!(d.events[0].tokens.output, 1_000_000);
    assert_eq!((&*d.events[0].host, d.machines), ("desk", 1));
}

/// A config whose only remote, `laptop`, answers with the file `export`.
fn with_remote(dir: &std::path::Path, export: &std::path::Path) -> Config {
    Config {
        paths: crate::config::PathsConfig { claude: vec![dir.join("claude")], codex: vec![dir.join("none")] },
        host: Some("desk".into()),
        remote: vec![crate::remote::RemoteConfig {
            name: Some("laptop".into()),
            // `export` and its flags land in $1.. and are ignored.
            command: ["sh", "-c", "cat \"$0\"", &export.to_string_lossy()].map(String::from).to_vec(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn remote_usage_merges_once_under_its_machine() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("claude/-x-proj");
    std::fs::create_dir_all(&logs).unwrap();
    let line = |id: &str, ts: &str| {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","sessionId":"s","requestId":"r","message":{{"id":"{id}","model":"claude-sonnet-4-5","usage":{{"input_tokens":1000,"output_tokens":10}}}}}}"#
        )
    };
    std::fs::write(logs.join("s.jsonl"), line("shared", "2026-11-01T10:00:00Z") + "\n").unwrap();

    // What the laptop exports: one response this machine also has (a
    // synced transcript) and one of its own.
    let export = dir.path().join("export.jsonl");
    let mut i = Interner::default();
    let laptop = |i: &mut Interner, id: &str, ts: i64| Event {
        id: i.get(id),
        model: i.get("claude-sonnet-4-5"),
        ..crate::aggregate::testutil::ev(i, crate::event::Tool::Claude, ts, "/home/z/web", "ls", 0.0)
    };
    let (shared, own) = (laptop(&mut i, "c:shared:r", 1), laptop(&mut i, "c:own:r", 1_900_000_000_000));
    let mut out = Vec::new();
    remote::write_export(&mut out, "sams-laptop", &[&shared, &own]).unwrap();
    std::fs::write(&export, out).unwrap();

    let cfg = with_remote(dir.path(), &export);
    let hist = dir.path().join("history.jsonl");
    let mut c = Collector::new(&cfg, Some(hist.clone()));
    c.load_history();
    c.scan();
    let st = c.sync_remotes();
    assert_eq!(st[0].error, None);
    assert_eq!((st[0].records, st[0].newest), (1, Some(1_900_000_000_000)), "the shared response stays local");
    assert!(st[0].synced.is_some());
    {
        let d = c.data.read().unwrap();
        let hosts: Vec<&str> = d.events.iter().map(|e| &*e.host).collect();
        assert_eq!(hosts, ["desk", "laptop"]);
        assert_eq!(d.machines, 2);
        assert!(d.events[1].priced, "remote records are priced here");
    }
    // Syncing again adds nothing, and the next sync starts near the newest record.
    assert_eq!(c.sync_remotes()[0].records, 1);
    assert_eq!(c.links[0].cursor.load(Ordering::Relaxed), 1_900_000_000_000);

    // The laptop's usage is kept: with the remote gone, history still has it.
    std::fs::remove_file(&export).unwrap();
    let mut c2 = Collector::new(&cfg, Some(hist));
    c2.load_history();
    let st = c2.sync_remotes();
    assert!(st[0].error.is_some(), "an unreachable remote reports why");
    assert_eq!(st[0].records, 1);
    let d = c2.data.read().unwrap();
    assert_eq!(d.events.iter().filter(|e| &*e.host == "laptop").count(), 1);
}

fn day_total(host: &str, day: &str, ts: i64, output: u64) -> t3::DayTotal {
    let tokens = crate::event::Tokens { input: 100, output, ..Default::default() };
    let total = t3::Total {
        host: host.into(),
        tool: Tool::Claude,
        model: "claude-sonnet-4-5".into(),
        day: day.into(),
        tokens,
        responses: 12,
        sessions: 2,
        cost: 4.5,
        unpriced: 0,
        agent: "claude".into(),
    };
    t3::DayTotal { total, ts }
}

#[test]
fn t3_machines_merge_without_counting_twice() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = with_remote(dir.path(), &dir.path().join("missing"));
    let hist = dir.path().join("history.jsonl");
    let mut c = Collector::new(&cfg, Some(hist.clone()));
    c.t3_roots_for = vec![Tool::Claude];
    let extra = dir.path().join("t3-claude/projects");
    std::fs::create_dir_all(&extra).unwrap();

    let machine = |id: &str, name: &str, result| t3::Machine { id: id.into(), name: name.into(), result };
    let read = |machines| t3::Outcome {
        version: "0.1.12".into(),
        roots: vec![(Tool::Claude, extra.clone()), (Tool::Codex, dir.path().join("t3-codex"))],
        local_hosts: vec!["Desk-Box.local".into()],
        machines,
        ..Default::default()
    };
    c.merge_t3(read(vec![
        machine(
            "e1",
            "Studio",
            Ok(vec![
                day_total("studio.local", "2026-11-14", 1000, 5),
                day_total("studio.local", "2026-11-15", 2000, 7),
            ]),
        ),
        // A link back to this very machine, and one to the ssh remote: both already held in detail.
        machine("e2", "Desk again", Ok(vec![day_total("Desk-Box.local", "2026-11-15", 2000, 9)])),
        machine("e3", "Laptop", Ok(vec![day_total("laptop.local", "2026-11-15", 2000, 9)])),
        machine("e4", "Server", Err("not reachable (asleep or offline)".into())),
    ]));
    {
        let d = c.data.read().unwrap();
        let shown: Vec<(&str, bool, usize, bool)> =
            d.remotes.iter().map(|r| (r.name.as_str(), r.target == VIA_T3, r.records, r.error.is_some())).collect();
        assert_eq!(shown, [("laptop", false, 0, false), ("Studio", true, 2, false), ("Server", true, 0, true)]);
        assert_eq!(d.events.len(), 2, "only Studio's totals are kept");
        assert!(d.events.iter().all(|e| &*e.host == "Studio" && e.priced && e.responses() == 12));
        assert!(d.events.iter().all(|e| e.cost == 4.5), "the cost T3 reported is kept");
        assert_eq!(d.t3, Some(T3Status { version: "0.1.12".into(), read: true, error: None }));
        // T3's Claude folder is read from now on; Codex's is fixed by the config.
        assert!(d.status[Tool::Claude.index()].roots.iter().any(|(p, _)| *p == extra));
        assert_eq!(d.status[Tool::Codex.index()].roots.len(), 1);
    }
    // A day that is read again replaces what was held, and history keeps the totals.
    c.merge_t3(read(vec![machine("e1", "Studio", Ok(vec![day_total("studio.local", "2026-11-15", 2000, 70)]))]));
    assert_eq!(c.data.read().unwrap().events.iter().map(|e| e.tokens.output).collect::<Vec<_>>(), [5, 70]);
    // Also when the new reading is lower, or only its cost changed.
    let mut repriced = day_total("studio.local", "2026-11-15", 2000, 60);
    repriced.total.cost = 9.0;
    c.merge_t3(read(vec![machine("e1", "Studio", Ok(vec![repriced.clone()]))]));
    c.merge_t3(read(vec![machine("e1", "Studio", Ok(vec![repriced]))]));
    {
        let d = c.data.read().unwrap();
        assert_eq!(d.events.iter().map(|e| (e.tokens.output, e.cost)).collect::<Vec<_>>(), [(5, 4.5), (60, 9.0)]);
    }
    let mut c2 = Collector::new(&cfg, Some(hist.clone()));
    c2.load_history();
    assert_eq!(
        c2.data.read().unwrap().events.iter().map(|e| (e.tokens.output, e.cost)).collect::<Vec<_>>(),
        [(5, 4.5), (60, 9.0)]
    );

    // Once Studio is also read over ssh, its daily totals step aside, now and on later runs.
    c.cover(["studio".to_string()]);
    let d = c.data.read().unwrap();
    assert!(d.events.is_empty());
    assert_eq!(d.remotes.iter().find(|r| r.name == "Studio").unwrap().records, 0);
    drop(d);
    let mut c3 = Collector::new(&cfg, Some(hist));
    c3.load_history();
    assert!(c3.data.read().unwrap().events.is_empty(), "remembered in machines.json");
}

#[test]
fn other_agents_come_from_t3_also_for_this_machine() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = with_remote(dir.path(), &dir.path().join("missing"));
    let hist = dir.path().join("history.jsonl");
    let mut c = Collector::new(&cfg, Some(hist.clone()));
    let grok = |host: &str, output: u64| {
        let mut t = day_total(host, "2026-11-15", 2000, output);
        t.total.tool = Tool::Grok;
        t.total.agent = "grok".into();
        t.total.model = "grok-build".into();
        t
    };
    let read = t3::Outcome {
        version: "0.1.12".into(),
        local_hosts: vec!["Desk-Box.local".into()],
        // This machine's own Grok usage, and the same reported back over a link to it.
        local: Some(Ok(vec![grok("Desk-Box.local", 5)])),
        machines: vec![
            t3::Machine { id: "e2".into(), name: "Desk again".into(), result: Ok(vec![grok("Desk-Box.local", 5)]) },
            t3::Machine { id: "e1".into(), name: "Studio".into(), result: Ok(vec![grok("studio.local", 8)]) },
        ],
        ..Default::default()
    };
    c.merge_t3(read);
    let d = c.data.read().unwrap();
    let seen: Vec<(&str, &str, Tool, u64)> =
        d.events.iter().map(|e| (&*e.host, &*e.project, e.tool, e.tokens.output)).collect();
    // Kept although this machine is read in detail (aitop has no Grok reader), and only once.
    assert_eq!(seen, [("desk", "Grok Build", Tool::Grok, 5), ("Studio", "Grok Build", Tool::Grok, 8)]);
    assert!(d.events.iter().all(|e| e.cost == 4.5 && e.priced));
    assert_eq!(d.status.len(), Tool::ALL.len());
    assert!(d.status[Tool::Grok.index()].roots.is_empty());
    drop(d);
    // It survives a restart through the history file.
    let mut c2 = Collector::new(&cfg, Some(hist));
    c2.load_history();
    assert_eq!(c2.data.read().unwrap().events.len(), 2);
}

#[test]
fn recounted_codex_sessions_replace_their_old_records() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = with_remote(dir.path(), &dir.path().join("missing"));
    let mut c = Collector::new(&cfg, None);
    let mut i = Interner::default();
    let codex = |i: &mut Interner, id: &str, session: &str, cost: f64| Event {
        id: i.get(id),
        ..crate::aggregate::testutil::ev(i, Tool::Codex, 1000, "/code/api", session, cost)
    };
    // History from an older aitop: two sessions, one of which no longer has logs.
    let old = vec![codex(&mut i, "x:kept:500", "kept", 1.0), codex(&mut i, "x:gone:900", "gone", 2.0)];
    c.merge(old, false);
    assert_eq!(c.data.read().unwrap().events.len(), 2);
    // The scan reads `kept` again, request by request.
    c.merge(vec![codex(&mut i, "x2:kept:1000:5.0.5:1", "kept", 0.5)], false);
    let ids = |c: &Collector| c.data.read().unwrap().events.iter().map(|e| e.id.to_string()).collect::<Vec<_>>();
    assert_eq!(ids(&c), ["x:gone:900", "x2:kept:1000:5.0.5:1"]);
    // Its old records stay out, also when history is loaded in one go.
    c.merge(vec![codex(&mut i, "x:kept:700", "kept", 1.0)], false);
    assert_eq!(ids(&c).len(), 2);
    let mut c2 = Collector::new(&cfg, None);
    let mixed = vec![codex(&mut i, "x:kept:500", "kept", 1.0), codex(&mut i, "x2:kept:1000:5.0.5:1", "kept", 0.5)];
    c2.merge(mixed, false);
    assert_eq!(ids(&c2), ["x2:kept:1000:5.0.5:1"]);
}

#[test]
fn read_only_leaves_history_alone() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("claude/-x-proj");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(
        logs.join("s.jsonl"),
        r#"{"type":"assistant","timestamp":"2026-11-01T10:00:00Z","sessionId":"s","requestId":"r","message":{"id":"m","model":"claude-sonnet-4-5","usage":{"input_tokens":1,"output_tokens":1}}}"#.to_string() + "\n",
    )
    .unwrap();
    let cfg = with_remote(dir.path(), &dir.path().join("missing"));
    let hist = dir.path().join("history.jsonl");
    let mut c = Collector::new(&cfg, Some(hist.clone()));
    c.read_only();
    c.load_history();
    c.scan();
    assert_eq!(c.data.read().unwrap().events.len(), 1);
    assert!(!hist.exists());
}
