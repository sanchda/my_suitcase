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

/// Current branch name; `None` when detached, unborn, or not a repo.
pub fn branch(dir: &Path) -> Option<String> {
    git(dir, &["symbolic-ref", "--short", "-q", "HEAD"])
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Is `old` an ancestor of `new`? True means the move was a pure fast-forward
/// (new commits only); false means history was rewritten (amend/reset/rebase).
pub fn is_ancestor(dir: &Path, old: &str, new: &str) -> bool {
    git(dir, &["merge-base", "--is-ancestor", old, new]).is_some()
}

/// Paths touched by any commit in `old..new` (per-commit, so an add-then-revert
/// inside the range still shows). Empty outside a repo or on error.
pub fn committed_paths(dir: &Path, old: &str, new: &str) -> Vec<String> {
    let range = format!("{old}..{new}");
    let out = match git(dir, &["log", "--name-only", "--format=", &range]) {
        Some(o) => o,
        None => return Vec::new(),
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Net textual diff plus a one-line-per-commit log for `old..new`, byte-bounded
/// for feeding to a judge model. Empty outside a repo.
pub fn range_diff_text(dir: &Path, old: &str, new: &str, max_bytes: usize) -> String {
    let range = format!("{old}..{new}");
    let mut out = String::new();
    if let Some(o) = git(dir, &["log", "--oneline", "--no-color", &range]) {
        out.push_str("Commits:\n");
        out.push_str(&String::from_utf8_lossy(&o.stdout));
        out.push('\n');
    }
    if let Some(o) = git(dir, &["diff", "--no-color", &range]) {
        out.push_str(&String::from_utf8_lossy(&o.stdout));
    }
    if out.len() > max_bytes {
        let mut end = max_bytes;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push_str("\n… (diff truncated)\n");
    }
    out
}

/// One contract breach found by the post-iteration audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breach {
    /// A fatal breach invalidates the loop's core invariant (wrong branch);
    /// non-fatal breaches count as no-progress and are surfaced loudly.
    pub fatal: bool,
    pub message: String,
}

/// Post-iteration audit of the prompt's commit/safety contract: branch
/// unchanged (fatal), HEAD moved fast-forward-only (no amend/reset), and no
/// `.ralph/`-style runtime paths committed. Lenient outside a repo or when a
/// "before" value is unknown.
pub fn audit_iteration(
    dir: &Path,
    branch_before: &Option<String>,
    head_before: &Option<String>,
    runtime_dir: &Path,
) -> Vec<Breach> {
    if !is_repo(dir) {
        return Vec::new();
    }
    let mut breaches = Vec::new();
    if let (Some(before), Some(now)) = (branch_before, branch(dir)) {
        if before != &now {
            breaches.push(Breach {
                fatal: true,
                message: format!("agent switched branches: `{before}` → `{now}`"),
            });
        }
    }
    if let (Some(before), Some(now)) = (head_before, head(dir)) {
        if before != &now {
            if !is_ancestor(dir, before, &now) {
                breaches.push(Breach {
                    fatal: false,
                    message: format!(
                        "history rewritten: HEAD moved {} → {} without fast-forward (amend/reset/rebase)",
                        &before[..before.len().min(12)],
                        &now[..now.len().min(12)]
                    ),
                });
            } else {
                // Only new commits can carry runtime-dir paths.
                let prefix = format!(
                    "{}/",
                    runtime_dir
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| ".ralph".into())
                );
                let staged: Vec<String> = committed_paths(dir, before, &now)
                    .into_iter()
                    .filter(|p| p.starts_with(&prefix))
                    .collect();
                if !staged.is_empty() {
                    breaches.push(Breach {
                        fatal: false,
                        message: format!(
                            "runtime files committed ({}): {}",
                            staged.len(),
                            staged.join(", ")
                        ),
                    });
                }
            }
        }
    }
    breaches
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
        // Hermetic against global config: a user-level commit.gpgsign=true would
        // otherwise make every test commit shell out to gpg.
        run(&dir, &["config", "commit.gpgsign", "false"]);
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
    fn audit_passes_clean_fast_forward_and_flags_breaches() {
        let dir = temp_repo();
        fs::write(dir.join("a.txt"), "1").unwrap();
        run(&dir, &["add", "a.txt"]);
        run(&dir, &["commit", "-qm", "one"]);
        run(&dir, &["branch", "-M", "work"]);
        let branch_before = branch(&dir);
        assert_eq!(branch_before.as_deref(), Some("work"));
        let head_before = head(&dir);
        let ralph_dir = dir.join(".ralph");

        // No movement at all → clean.
        assert!(audit_iteration(&dir, &branch_before, &head_before, &ralph_dir).is_empty());

        // Fast-forward commit of product code → clean.
        fs::write(dir.join("a.txt"), "2").unwrap();
        run(&dir, &["add", "a.txt"]);
        run(&dir, &["commit", "-qm", "two"]);
        assert!(audit_iteration(&dir, &branch_before, &head_before, &ralph_dir).is_empty());

        // Commit that stages runtime files → non-fatal breach.
        fs::create_dir_all(dir.join(".ralph")).unwrap();
        fs::write(dir.join(".ralph/BACKLOG.md"), "x").unwrap();
        run(&dir, &["add", "-f", ".ralph/BACKLOG.md"]);
        run(&dir, &["commit", "-qm", "oops runtime"]);
        let breaches = audit_iteration(&dir, &branch_before, &head_before, &ralph_dir);
        assert_eq!(breaches.len(), 1, "{breaches:?}");
        assert!(!breaches[0].fatal);
        assert!(breaches[0].message.contains(".ralph/BACKLOG.md"));

        // Amend (history rewrite) → non-fatal breach.
        let head_pre_amend = head(&dir);
        run(&dir, &["commit", "-q", "--amend", "-m", "oops amended"]);
        let breaches = audit_iteration(&dir, &branch_before, &head_pre_amend, &ralph_dir);
        assert_eq!(breaches.len(), 1, "{breaches:?}");
        assert!(!breaches[0].fatal);
        assert!(breaches[0].message.contains("history rewritten"));

        // Branch switch → fatal breach.
        run(&dir, &["checkout", "-qb", "rogue"]);
        let breaches = audit_iteration(&dir, &branch_before, &head(&dir), &ralph_dir);
        assert_eq!(breaches.len(), 1, "{breaches:?}");
        assert!(breaches[0].fatal);
        assert!(breaches[0].message.contains("rogue"));
    }

    #[test]
    fn range_diff_text_is_bounded() {
        let dir = temp_repo();
        fs::write(dir.join("a.txt"), "line\n".repeat(2000)).unwrap();
        run(&dir, &["add", "a.txt"]);
        run(&dir, &["commit", "-qm", "base"]);
        let old = head(&dir).unwrap();
        fs::write(dir.join("a.txt"), "other\n".repeat(2000)).unwrap();
        run(&dir, &["add", "a.txt"]);
        run(&dir, &["commit", "-qm", "big change"]);
        let new = head(&dir).unwrap();
        let text = range_diff_text(&dir, &old, &new, 2_000);
        assert!(text.len() <= 2_100, "was {}", text.len());
        assert!(text.contains("truncated"));
        assert!(text.contains("big change"));
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
}
