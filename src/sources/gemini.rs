//! Gemini CLI: `~/.gemini/tmp/<project>/chats/session-*.json`.
//!
//! A session file is a JSON document `{sessionId, messages: [...]}` that is
//! rewritten as the chat grows. Messages of type `gemini` carry
//! `tokens: {input, output, cached, thoughts, tool, total}` where `input`
//! includes `cached`. The `<project>` directory is either a readable name,
//! or the SHA-256 of the project path (older versions, with a
//! `.project_root` file next to it in some releases).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

use super::{file_stamp, parse_ts, read_new_lines, walk, Ctx, Source, SourceStatus, Tail};
use crate::event::{Event, Tokens, Tool};

pub struct GeminiSource {
    roots: Vec<PathBuf>,
    files: HashMap<PathBuf, Tail>,
    projects: HashMap<PathBuf, (Arc<str>, Arc<str>)>,
}

impl GeminiSource {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        GeminiSource { roots, files: HashMap::new(), projects: HashMap::new() }
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Session {
    session_id: Option<String>,
    start_time: Option<String>,
    messages: Vec<Message>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Message {
    id: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    model: Option<String>,
    tokens: Option<GTokens>,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(default)]
struct GTokens {
    input: u64,
    output: u64,
    cached: u64,
    thoughts: u64,
    tool: u64,
}

fn is_session_file(p: &Path) -> bool {
    let in_chats = p.parent().and_then(|d| d.file_name()).is_some_and(|n| n == "chats");
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    in_chats && name.starts_with("session-") && (name.ends_with(".json") || name.ends_with(".jsonl"))
}

impl Source for GeminiSource {
    fn scan(&mut self, ctx: &mut Ctx, out: &mut Vec<Event>) -> SourceStatus {
        let mut status = SourceStatus::new(Tool::Gemini, &self.roots);
        let mut paths = Vec::new();
        for root in &self.roots {
            walk(root, &is_session_file, &mut paths);
        }
        status.files = paths.len();
        let present: std::collections::HashSet<&PathBuf> = paths.iter().collect();
        self.files.retain(|p, _| present.contains(p));
        for path in &paths {
            let project_dir = path.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
            let project = match self.projects.get(&project_dir) {
                Some(p) => p.clone(),
                None => {
                    let p = resolve_project(&project_dir, ctx);
                    self.projects.insert(project_dir.clone(), p.clone());
                    p
                }
            };
            let tail = self.files.entry(path.clone()).or_default();
            let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
            if path.extension().is_some_and(|e| e == "jsonl") {
                let mut session = stem.clone();
                if let Err(e) = read_new_lines(path, tail, |line| {
                    if let Ok(s) = serde_json::from_slice::<Session>(line) {
                        if let Some(id) = s.session_id {
                            session = id;
                        }
                    }
                    if let Ok(m) = serde_json::from_slice::<Message>(line) {
                        if let Some(e) = to_event(&m, &session, None, &project, ctx) {
                            out.push(e);
                        }
                    }
                }) {
                    status.errors.push(format!("{}: {e}", path.display()));
                }
                continue;
            }
            // Whole-document format: re-parse when the file changes.
            let Some((len, mtime)) = file_stamp(path) else { continue };
            if tail.len == len && tail.mtime == mtime {
                continue;
            }
            tail.len = len;
            tail.mtime = mtime;
            let text = match std::fs::read(path) {
                Ok(t) => t,
                Err(e) => {
                    status.errors.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            let Ok(s) = serde_json::from_slice::<Session>(&text) else {
                // Probably caught mid-write; retry next scan.
                tail.mtime = 0;
                continue;
            };
            let session = s.session_id.clone().unwrap_or(stem);
            let start = s.start_time.as_deref().and_then(parse_ts).or(Some(mtime));
            for m in &s.messages {
                if let Some(e) = to_event(m, &session, start, &project, ctx) {
                    out.push(e);
                }
            }
        }
        status
    }
}

fn to_event(
    m: &Message,
    session: &str,
    fallback_ts: Option<i64>,
    project: &(Arc<str>, Arc<str>),
    ctx: &mut Ctx,
) -> Option<Event> {
    if m.kind.as_deref() != Some("gemini") {
        return None;
    }
    let t = m.tokens?;
    let cached = t.cached.min(t.input);
    let tokens = Tokens {
        input: t.input - cached + t.tool,
        output: t.output + t.thoughts,
        cache_read: cached,
        cache_write: 0,
        cache_write_1h: 0,
        reasoning: t.thoughts,
    };
    if tokens.is_zero() {
        return None;
    }
    let ts = m.timestamp.as_deref().and_then(parse_ts).or(fallback_ts)?;
    let id = format!("g:{session}:{}", m.id.as_deref().unwrap_or(m.timestamp.as_deref().unwrap_or("")));
    Some(Event {
        tool: Tool::Gemini,
        id: ctx.interner.get(&id),
        ts,
        session: ctx.interner.get(session),
        project: project.0.clone(),
        project_path: project.1.clone(),
        model: ctx.interner.get(m.model.as_deref().unwrap_or("unknown")),
        tokens,
        fast: false,
        cost: 0.0,
        priced: false,
    })
}

fn resolve_project(dir: &Path, ctx: &mut Ctx) -> (Arc<str>, Arc<str>) {
    let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let root_file =
        std::fs::read_to_string(dir.join(".project_root")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(root) = root_file {
        return ctx.projects.resolve(&root, ctx.interner);
    }
    let is_hash = name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit());
    if is_hash {
        if let Some(path) = ctx.projects.reverse_gemini_hash(&name) {
            return ctx.projects.resolve(&path, ctx.interner);
        }
        let short = format!("#{}", &name[..8]);
        return (ctx.interner.get(&short), ctx.interner.get(&name));
    }
    (ctx.interner.get(&name), ctx.interner.get(&dir.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Interner;
    use crate::project::{sha256_hex, ProjectResolver};

    #[test]
    fn parses_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let hash = sha256_hex("/nowhere/research");
        let chats = dir.path().join(&hash).join("chats");
        std::fs::create_dir_all(&chats).unwrap();
        let doc = r#"{"sessionId":"g1","projectHash":"x","startTime":"2026-11-01T09:00:00Z","messages":[
          {"id":"u1","timestamp":"2026-11-01T09:00:01Z","type":"user","content":"hello there"},
          {"id":"a1","timestamp":"2026-11-01T09:00:05Z","type":"gemini","content":"hi","model":"gemini-2.5-pro",
           "tokens":{"input":1000,"output":50,"cached":400,"thoughts":30,"tool":10,"total":1090}},
          {"id":"a2","timestamp":"2026-11-01T09:00:09Z","type":"gemini","content":"no tokens"},
          {"id":"i1","timestamp":"2026-11-01T09:00:10Z","type":"info","content":"x"}]}"#;
        std::fs::write(chats.join("session-2026-11-01T09-00-g1.json"), doc).unwrap();
        std::fs::write(chats.join("not-a-session.json"), "{}").unwrap();

        let mut src = GeminiSource::new(vec![dir.path().to_path_buf()]);
        let mut interner = Interner::default();
        let mut projects = ProjectResolver::new();
        projects.resolve("/nowhere/research", &mut interner);
        let mut ctx = Ctx { interner: &mut interner, projects: &mut projects };
        let mut out = Vec::new();
        let st = src.scan(&mut ctx, &mut out);
        assert_eq!(st.files, 1);
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(
            e.tokens,
            Tokens { input: 610, output: 80, cache_read: 400, cache_write: 0, cache_write_1h: 0, reasoning: 30 }
        );
        assert_eq!(&*e.project, "research");
        assert_eq!(&*e.model, "gemini-2.5-pro");
        out.clear();
        src.scan(&mut ctx, &mut out);
        assert!(out.is_empty(), "unchanged file is not re-read");
    }
}
