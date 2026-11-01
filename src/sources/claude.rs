//! Claude Code: `~/.claude/projects/<encoded-cwd>/<session>.jsonl`, plus
//! sub-agent transcripts in `<session>/subagents/*.jsonl`.
//!
//! Every assistant response is logged with its `message.usage`. A streamed
//! response is written as several lines (one per content block) that share
//! `message.id` + `requestId` and repeat the usage, so lines are
//! de-duplicated on that pair, keeping the one with the most output.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{discover, parse_ts, prune, Change, Ctx, Source, SourceStatus, Tail};
use crate::event::{Event, Tokens, Tool};
use crate::paths::{expand, home};
use crate::project::decode_claude_dir;

pub struct ClaudeSource {
    roots: Vec<PathBuf>,
    files: HashMap<PathBuf, Tail>,
}

impl ClaudeSource {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        ClaudeSource { roots, files: HashMap::new() }
    }
}

/// `$CLAUDE_CONFIG_DIR/projects` (comma-separated list allowed), otherwise
/// both the current (`~/.config/claude`) and legacy (`~/.claude`) locations.
pub fn default_roots() -> Vec<PathBuf> {
    if let Ok(dirs) = std::env::var("CLAUDE_CONFIG_DIR") {
        let v: Vec<PathBuf> = dirs
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| expand(Path::new(s)).join("projects"))
            .collect();
        if !v.is_empty() {
            return v;
        }
    }
    vec![home().join(".config/claude/projects"), home().join(".claude/projects")]
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    session_id: Option<String>,
    request_id: Option<String>,
    uuid: Option<String>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
    cache_creation: Option<CacheCreation>,
    speed: Option<String>,
    output_tokens_details: Option<OutputDetails>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CacheCreation {
    ephemeral_5m_input_tokens: u64,
    ephemeral_1h_input_tokens: u64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OutputDetails {
    thinking_tokens: u64,
}

impl Source for ClaudeSource {
    fn tool(&self) -> Tool {
        Tool::Claude
    }

    fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    fn scan(&mut self, ctx: &mut Ctx, out: &mut Vec<Event>) -> SourceStatus {
        let mut status = SourceStatus::new(Tool::Claude, &self.roots);
        let paths = discover(&self.roots, |p| p.extension().is_some_and(|e| e == "jsonl"));
        status.files = paths.len();
        prune(&mut self.files, &paths);
        for path in paths {
            let tail = self.files.entry(path.clone()).or_default();
            // Transcripts are append-only, and re-read lines are de-duplicated
            // by the collector, so a rewrite needs no special handling.
            match tail.poll(&path) {
                Ok(Change::Unchanged) => continue,
                Ok(_) => {}
                Err(e) => {
                    status.error(&path, e);
                    continue;
                }
            }
            let (fallback_session, fallback_project) = path_hints(&path, &self.roots);
            let res = tail.read_lines(&path, |line| {
                if let Some(e) = parse_line(line, &fallback_session, &fallback_project, ctx) {
                    out.push(e);
                }
            });
            if let Err(e) = res {
                status.error(&path, e);
            }
        }
        status
    }
}

/// Session id and project name implied by the file location.
fn path_hints(path: &Path, roots: &[PathBuf]) -> (String, String) {
    let parent = path.parent();
    let session = if parent.and_then(|p| p.file_name()).is_some_and(|n| n == "subagents") {
        parent.and_then(|p| p.parent()).and_then(|p| p.file_name())
    } else {
        path.file_stem()
    };
    let session = session.map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let project = roots
        .iter()
        .find_map(|r| path.strip_prefix(r).ok())
        .and_then(|rel| rel.components().next())
        .map(|c| decode_claude_dir(&c.as_os_str().to_string_lossy()))
        .unwrap_or_else(|| "unknown".into());
    (session, project)
}

fn parse_line(line: &[u8], fallback_session: &str, fallback_project: &str, ctx: &mut Ctx) -> Option<Event> {
    // Cheap pre-filter: most lines (user turns, tool results, summaries) carry no usage.
    let text = std::str::from_utf8(line).ok()?;
    if !text.contains("\"usage\"") {
        return None;
    }
    let l: Line = serde_json::from_str(text).ok()?;
    if l.kind.as_deref().is_some_and(|k| k != "assistant") {
        return None;
    }
    let msg = l.message?;
    let u = msg.usage?;
    let model = msg.model.unwrap_or_default();
    if model == "<synthetic>" {
        return None;
    }
    let (cw5, cw1h) = match &u.cache_creation {
        Some(c) if c.ephemeral_5m_input_tokens + c.ephemeral_1h_input_tokens > 0 => {
            (c.ephemeral_5m_input_tokens, c.ephemeral_1h_input_tokens)
        }
        _ => (u.cache_creation_input_tokens, 0),
    };
    let tokens = Tokens {
        input: u.input_tokens,
        output: u.output_tokens,
        cache_read: u.cache_read_input_tokens,
        cache_write: cw5,
        cache_write_1h: cw1h,
        reasoning: u.output_tokens_details.map(|d| d.thinking_tokens).unwrap_or(0),
    };
    if tokens.is_zero() {
        return None;
    }
    let ts = l.timestamp.as_deref().and_then(parse_ts)?;
    let id = match (&msg.id, &l.request_id) {
        (Some(m), Some(r)) => format!("c:{m}:{r}"),
        (Some(m), None) => format!("c:{m}"),
        _ => format!("c:{}", l.uuid.as_deref().unwrap_or("")),
    };
    let session = l.session_id.unwrap_or_else(|| fallback_session.to_string());
    let (project, project_path) = match l.cwd.as_deref() {
        Some(cwd) if !cwd.is_empty() => ctx.projects.resolve(cwd, ctx.interner),
        _ => (ctx.interner.get(fallback_project), ctx.interner.get(fallback_project)),
    };
    Some(Event {
        tool: Tool::Claude,
        id: ctx.interner.get(&id),
        ts,
        session: ctx.interner.get(&session),
        project,
        project_path,
        model: ctx.interner.get(&model),
        tokens,
        fast: u.speed.as_deref() == Some("fast"),
        cost: 0.0,
        priced: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Interner;
    use crate::project::ProjectResolver;

    fn line(id: &str, req: &str, out: u64, extra: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-11-01T10:00:00.000Z","cwd":"/nonexistent/proj","sessionId":"s1","requestId":"{req}","message":{{"id":"{id}","model":"claude-opus-5-5","content":[{{"type":"text","text":"hi \"usage\""}}],"usage":{{"input_tokens":2,"output_tokens":{out},"cache_creation_input_tokens":100,"cache_read_input_tokens":1000{extra}}}}}}}"#
        )
    }

    #[test]
    fn parses_usage_lines() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("-nonexistent-proj");
        std::fs::create_dir_all(proj.join("s1/subagents")).unwrap();
        let body = [
            r#"{"type":"user","message":{"role":"user","content":"hello"}}"#.to_string(),
            line("m1", "r1", 10, ""),
            line("m1", "r1", 50, ""),
            line("m2", "r2", 7, r#","cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":100},"speed":"fast","output_tokens_details":{"thinking_tokens":5}"#),
            r#"{"type":"assistant","timestamp":"2026-11-01T10:00:00Z","message":{"id":"x","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#.to_string(),
            "not json".to_string(),
        ]
        .join("\n")
            + "\n";
        std::fs::write(proj.join("s1.jsonl"), body).unwrap();
        std::fs::write(proj.join("s1/subagents/agent-a.jsonl"), line("m3", "r3", 1, "") + "\n").unwrap();

        let mut src = ClaudeSource::new(vec![dir.path().to_path_buf()]);
        let mut interner = Interner::default();
        let mut projects = ProjectResolver::new();
        let mut ctx = Ctx { interner: &mut interner, projects: &mut projects };
        let mut out = Vec::new();
        let st = src.scan(&mut ctx, &mut out);
        assert_eq!(st.files, 2);
        // duplicates are emitted; the collector keeps the largest
        assert_eq!(out.len(), 4);
        let m2 = out.iter().find(|e| e.id.contains("m2")).unwrap();
        assert_eq!(m2.tokens.cache_write_1h, 100);
        assert_eq!(m2.tokens.cache_write, 0);
        assert_eq!(m2.tokens.reasoning, 5);
        assert!(m2.fast);
        assert_eq!(&*m2.project, "proj");
        assert!(out.iter().all(|e| &*e.session == "s1"));

        // nothing new on rescan
        out.clear();
        src.scan(&mut ctx, &mut out);
        assert!(out.is_empty());
    }
}
