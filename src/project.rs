//! Map working directories to projects.
//!
//! A session's cwd is resolved to the root of its git repository, so work
//! done in `repo/src` and `repo/docs` is counted together, and git worktrees
//! (including Claude Code's `.claude/worktrees/*`) are folded into the main
//! repository.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::event::Interner;

#[derive(Default)]
pub struct ProjectResolver {
    cache: HashMap<String, (Arc<str>, Arc<str>)>,
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
        let v = (interner.get(&name), interner.get(&root_s));
        self.cache.insert(cwd, v.clone());
        v
    }
}

fn normalize(p: &str) -> String {
    let t = p.trim();
    let t = if t.len() > 1 { t.trim_end_matches(['/', '\\']) } else { t };
    t.to_string()
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
    fn decodes_claude_dir() {
        assert_eq!(decode_claude_dir("-home-user-aitop"), "aitop");
    }
}
