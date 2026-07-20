//! Git guardrails: a loop-start baseline of tracked dirt, a per-iteration
//! productivity check (did a new commit land?), and a newly-dirty warning.
//!
//! Every function takes the working directory to run in (via `git -C`), so the
//! control loop can point at the repo root and tests can point at a temp repo.
//! All functions degrade safely outside a git repo: productivity returns `true`
//! (we can't judge, so never false-flag no-progress) and the dirty count is 0.

use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
}

/// Are we inside a git work tree at `dir`?
pub fn is_repo(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--git-dir"]).is_some()
}

/// Current HEAD commit sha, if any.
pub fn head(dir: &Path) -> Option<String> {
    git(dir, &["rev-parse", "HEAD"]).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Did this iteration make progress in git terms — i.e. did HEAD advance since
/// `before`? A committed iteration (the prompt's contract) advances HEAD. When
/// not a repo, returns `true` so productivity is never falsely denied.
pub fn advanced_since(dir: &Path, before: &Option<String>) -> bool {
    if !is_repo(dir) {
        return true;
    }
    match (before, head(dir)) {
        (Some(b), Some(now)) => &now != b,
        // No HEAD before or after (e.g. no commits yet) — can't confirm; lenient.
        _ => true,
    }
}

/// `git status --porcelain --untracked-files=no`, sorted lines.
fn tracked_dirt(dir: &Path) -> Vec<String> {
    let out = match git(dir, &["status", "--porcelain", "--untracked-files=no"]) {
        Some(o) => o,
        None => return Vec::new(),
    };
    let mut lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    lines.sort();
    lines
}

/// Snapshot tracked dirt to `baseline` (best-effort; no-op outside a repo).
pub fn write_baseline(dir: &Path, baseline: &Path) {
    if !is_repo(dir) {
        return;
    }
    let _ = std::fs::write(baseline, tracked_dirt(dir).join("\n"));
}

/// Count tracked files dirty now but not in the baseline — warns that an
/// iteration may have skipped its commit. Pre-existing operator dirt in the
/// baseline must not cry wolf.
pub fn newly_dirty(dir: &Path, baseline: &Path) -> usize {
    let base: std::collections::HashSet<String> = std::fs::read_to_string(baseline)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    tracked_dirt(dir)
        .into_iter()
        .filter(|l| !base.contains(l))
        .count()
}

/// Is `path` tracked by git in the work tree at `dir`?
pub fn is_tracked(dir: &Path, path: &Path) -> bool {
    git(
        dir,
        &["ls-files", "--error-unmatch", &path.to_string_lossy()],
    )
    .is_some()
}

/// `git mv from to` then commit only that move with `msg`. Best-effort: returns
/// whether both steps succeeded; leaves the tree as git left it on failure.
pub fn mv_and_commit(dir: &Path, from: &Path, to: &Path, msg: &str) -> bool {
    let from = from.to_string_lossy();
    let to = to.to_string_lossy();
    if git(dir, &["mv", &from, &to]).is_none() {
        return false;
    }
    git(dir, &["commit", "-m", msg, "--", &to, &from]).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn run(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git")
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    }

    fn temp_repo() -> PathBuf {
        // Unique per call (not just per process): cargo runs tests in parallel
        // threads within one process, and std::process::id() alone would let
        // concurrent tests collide on the same directory.
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ralph-git-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q"]);
        run(&dir, &["config", "user.email", "t@t"]);
        run(&dir, &["config", "user.name", "t"]);
        dir
    }

    #[test]
    fn non_repo_is_lenient() {
        let dir = std::env::temp_dir().join(format!("ralph-nonrepo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(!is_repo(&dir));
        assert!(advanced_since(&dir, &Some("x".into()))); // lenient outside a repo
        assert_eq!(newly_dirty(&dir, &dir.join("nope")), 0);
    }

    #[test]
    fn detects_new_commit_and_dirt() {
        let dir = temp_repo();
        fs::write(dir.join("a.txt"), "1").unwrap();
        run(&dir, &["add", "a.txt"]);
        run(&dir, &["commit", "-qm", "one"]);
        let before = head(&dir);
        assert!(before.is_some());

        // No new commit yet, but modify the tracked file → not advanced, 1 dirty.
        fs::write(dir.join("a.txt"), "2").unwrap();
        assert!(!advanced_since(&dir, &before));
        let baseline = dir.join("baseline");
        write_baseline(&dir, &baseline); // baseline captures the current dirt...
        assert_eq!(newly_dirty(&dir, &baseline), 0); // ...so nothing is "newly" dirty

        // Baseline taken clean, then dirty → newly_dirty sees it.
        run(&dir, &["checkout", "--", "a.txt"]);
        write_baseline(&dir, &baseline);
        fs::write(dir.join("a.txt"), "3").unwrap();
        assert_eq!(newly_dirty(&dir, &baseline), 1);

        // Commit → HEAD advances.
        run(&dir, &["add", "a.txt"]);
        run(&dir, &["commit", "-qm", "two"]);
        assert!(advanced_since(&dir, &before));
    }

    #[test]
    fn track_check_and_move_commit() {
        let dir = temp_repo();
        fs::create_dir_all(dir.join("archive")).unwrap();
        fs::write(dir.join("BACKLOG.md"), "work").unwrap();
        // Untracked yet.
        assert!(!is_tracked(&dir, &dir.join("BACKLOG.md")));
        run(&dir, &["add", "BACKLOG.md"]);
        run(&dir, &["commit", "-qm", "add backlog"]);
        assert!(is_tracked(&dir, &dir.join("BACKLOG.md")));

        let before = head(&dir);
        let dest = dir.join("archive/BACKLOG-x.md");
        assert!(mv_and_commit(
            &dir,
            &dir.join("BACKLOG.md"),
            &dest,
            "archive"
        ));
        // File moved, HEAD advanced, tracked tree clean.
        assert!(!dir.join("BACKLOG.md").exists());
        assert!(dest.exists());
        assert!(advanced_since(&dir, &before));
        assert!(tracked_dirt(&dir).is_empty());
    }
}
