//! Map working directories to projects.
//!
//! A session's cwd is resolved to the root of its git repository, so work
//! done in `repo/src` and `repo/docs` is counted together, and git worktrees
//! (including Claude Code's `.claude/worktrees/*`) are folded into the main
//! repository.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::event::Interner;

#[derive(Default)]
pub struct ProjectResolver {
    cache: HashMap<String, (Arc<str>, Arc<str>)>,
    /// Every path seen so far; candidates for reversing Gemini's hashed dirs.
    known: HashSet<String>,
    hashes: HashMap<String, String>,
    hashed_count: usize,
    home_indexed: bool,
    home: Option<PathBuf>,
}

impl ProjectResolver {
    pub fn new() -> Self {
        ProjectResolver { home: dirs::home_dir(), ..Default::default() }
    }

    /// Resolve a working directory into `(display name, root path)`.
    pub fn resolve(&mut self, cwd: &str, interner: &mut Interner) -> (Arc<str>, Arc<str>) {
        let cwd = normalize(cwd);
        if let Some(hit) = self.cache.get(&cwd) {
            return hit.clone();
        }
        let root = git_root(Path::new(&cwd), self.home.as_deref()).unwrap_or_else(|| PathBuf::from(&cwd));
        let root_s = root.to_string_lossy().to_string();
        let name = display_name(&root, self.home.as_deref());
        self.known.insert(cwd.clone());
        self.known.insert(root_s.clone());
        let v = (interner.get(&name), interner.get(&root_s));
        self.cache.insert(cwd, v.clone());
        v
    }

    /// Gemini CLI names its per-project directories after the SHA-256 of the
    /// project path. Try to find which known directory produced `hash`.
    pub fn reverse_gemini_hash(&mut self, hash: &str) -> Option<String> {
        if self.hashed_count != self.known.len() {
            for p in &self.known {
                self.hashes.entry(sha256_hex(p)).or_insert_with(|| p.clone());
            }
            self.hashed_count = self.known.len();
        }
        if let Some(p) = self.hashes.get(hash) {
            return Some(p.clone());
        }
        if !self.home_indexed {
            self.home_indexed = true;
            if let Some(home) = self.home.clone() {
                for p in candidate_dirs(&home) {
                    let s = p.to_string_lossy().to_string();
                    self.hashes.entry(sha256_hex(&s)).or_insert(s);
                }
            }
            return self.hashes.get(hash).cloned();
        }
        None
    }
}

fn normalize(p: &str) -> String {
    let t = p.trim();
    let t = if t.len() > 1 { t.trim_end_matches(['/', '\\']) } else { t };
    t.to_string()
}

pub fn sha256_hex(s: &str) -> String {
    let d = Sha256::digest(s.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Walk up from `dir` looking for `.git`. Never treats the home directory
/// itself as a repository (dotfile repos would otherwise swallow everything).
fn git_root(dir: &Path, home: Option<&Path>) -> Option<PathBuf> {
    for anc in dir.ancestors() {
        if Some(anc) == home && anc != dir {
            return None;
        }
        let git = anc.join(".git");
        let Ok(meta) = std::fs::metadata(&git) else { continue };
        if meta.is_dir() {
            return Some(anc.to_path_buf());
        }
        // A worktree: `.git` is a file containing `gitdir: /repo/.git/worktrees/x`.
        if let Ok(text) = std::fs::read_to_string(&git) {
            if let Some(gitdir) = text.trim().strip_prefix("gitdir:") {
                let gitdir = gitdir.trim();
                if let Some(i) = gitdir.find("/.git/worktrees/") {
                    return Some(PathBuf::from(&gitdir[..i]));
                }
            }
        }
        return Some(anc.to_path_buf());
    }
    None
}

fn display_name(root: &Path, home: Option<&Path>) -> String {
    if Some(root) == home {
        return "~".into();
    }
    match root.file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => root.to_string_lossy().to_string(),
    }
}

/// Directories up to two levels below home (skipping hidden ones), used to
/// reverse Gemini project hashes when the project was never seen elsewhere.
fn candidate_dirs(home: &Path) -> Vec<PathBuf> {
    let mut out = vec![home.to_path_buf()];
    let mut level = vec![home.to_path_buf()];
    for _ in 0..2 {
        let mut next = Vec::new();
        for d in &level {
            let Ok(rd) = std::fs::read_dir(d) else { continue };
            for e in rd.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') || name == "node_modules" || name == "Library" {
                    continue;
                }
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    next.push(e.path());
                    if out.len() + next.len() > 5000 {
                        break;
                    }
                }
            }
        }
        out.extend(next.iter().cloned());
        level = next;
    }
    out
}

/// Claude Code encodes project paths as directory names by replacing `/`
/// (and `.`) with `-`. The encoding is lossy, so this is only a fallback.
pub fn decode_claude_dir(name: &str) -> String {
    name.rsplit('-').find(|s| !s.is_empty()).unwrap_or(name).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_git_root_and_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("myrepo");
        std::fs::create_dir_all(repo.join(".git/worktrees/feature")).unwrap();
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        let wt = tmp.path().join("wt-feature");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}/.git/worktrees/feature\n", repo.display())).unwrap();

        let mut r = ProjectResolver::new();
        let mut i = Interner::default();
        let (name, root) = r.resolve(repo.join("src/deep").to_str().unwrap(), &mut i);
        assert_eq!(&*name, "myrepo");
        assert_eq!(&*root, repo.to_str().unwrap());
        let (name, _) = r.resolve(&format!("{}/", wt.display()), &mut i);
        assert_eq!(&*name, "myrepo");
        // non-existent dirs fall back to their basename
        let (name, _) = r.resolve("/does/not/exist/proj", &mut i);
        assert_eq!(&*name, "proj");
    }

    #[test]
    fn reverses_gemini_hash() {
        let mut r = ProjectResolver::new();
        let mut i = Interner::default();
        r.resolve("/no/such/place/alpha", &mut i);
        let h = sha256_hex("/no/such/place/alpha");
        assert_eq!(r.reverse_gemini_hash(&h).as_deref(), Some("/no/such/place/alpha"));
    }

    #[test]
    fn decodes_claude_dir() {
        assert_eq!(decode_claude_dir("-home-user-aitop"), "aitop");
    }
}
