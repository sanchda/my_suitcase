//! Branch + linked-worktree detection for a directory. Best-effort: any git
//! failure (not a repo, git absent) yields all-None.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitInfo { pub branch: Option<String>, pub worktree: Option<String> }

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn git_info(dir: &Path) -> GitInfo {
    let branch = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]);
    // A linked worktree has --git-dir ending in `.../worktrees/<name>` and a
    // --git-common-dir that differs from it.
    let git_dir = git(dir, &["rev-parse", "--git-dir"]);
    let common = git(dir, &["rev-parse", "--git-common-dir"]);
    let worktree = match (&git_dir, &common) {
        (Some(g), Some(c)) if g != c => Path::new(g).file_name().and_then(|s| s.to_str()).map(String::from),
        _ => None,
    };
    GitInfo { branch, worktree }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn run(dir: &Path, prog: &str, args: &[&str]) {
        let ok = Command::new(prog).arg("-C").arg(dir).args(args).status().map(|s| s.success()).unwrap_or(false);
        assert!(ok, "{prog} {args:?} failed in {dir:?}");
    }

    #[test]
    fn plain_repo_reports_branch_no_worktree() {
        let dir = std::env::temp_dir().join(format!("ctop-git-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, "git", &["init", "-q", "-b", "main"]);
        run(&dir, "git", &["config", "user.email", "t@t"]);
        run(&dir, "git", &["config", "user.name", "t"]);
        std::fs::write(dir.join("f"), "x").unwrap();
        run(&dir, "git", &["add", "."]);
        run(&dir, "git", &["commit", "-q", "-m", "init"]);
        let gi = git_info(&dir);
        assert_eq!(gi.branch.as_deref(), Some("main"));
        assert_eq!(gi.worktree, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn non_repo_is_all_none() {
        let dir = std::env::temp_dir().join(format!("ctop-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(git_info(&dir), GitInfo::default());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn linked_worktree_reports_worktree_name() {
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("ctop-wt-main-{}", pid));
        let wt_dir = std::env::temp_dir().join(format!("ctop-wt-wt-{}", pid));
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, "git", &["init", "-q", "-b", "main"]);
        run(&dir, "git", &["config", "user.email", "t@t"]);
        run(&dir, "git", &["config", "user.name", "t"]);
        std::fs::write(dir.join("f"), "x").unwrap();
        run(&dir, "git", &["add", "."]);
        run(&dir, "git", &["commit", "-q", "-m", "init"]);
        run(&dir, "git", &["worktree", "add", wt_dir.to_str().unwrap(), "-b", "wt-branch"]);
        let gi = git_info(&wt_dir);
        assert_eq!(gi.branch.as_deref(), Some("wt-branch"));
        assert!(gi.worktree.is_some());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&wt_dir).ok();
    }
}
