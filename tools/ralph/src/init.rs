//! `ralph init` — scaffold the `.ralph/` home in the current repo. Idempotent:
//! never overwrites an existing file; reports each path as created or skipped.

use crate::R;
use std::fs;
use std::path::Path;

const PROMPT_TEMPLATE: &str = include_str!("../PROMPT.template.md");

const GITIGNORE_SENTINEL: &str = "# ralph loop home (managed by `ralph init`)";
const GITIGNORE_BLOCK: &str = "\
# ralph loop home (managed by `ralph init`)
/.ralph/*
!/.ralph/PROMPT.md
!/.ralph/ralph.toml
!/.ralph/VISION.md
!/.ralph/BACKLOG.md
!/.ralph/PROGRESS.md
!/.ralph/archive/
";

const RALPH_TOML_STUB: &str = "\
# ralph config — all keys optional; uncomment to override defaults.
# See `ralph --help` and tools/ralph/README.md.
# model = \"sonnet\"
# fallback_model = \"sonnet\"
# max_cost_usd = 25.0
# max_duration = \"8h\"
# iteration_timeout = \"45m\"
# escalate_after = 2
# abort_after = 4
# backlog = \".ralph/BACKLOG.md\"
# extra_args = [\"--add-dir\", \"/some/path\"]
";

const BACKLOG_STUB: &str = "\
# Backlog

Ordered work list — the loop takes the first unfinished item unless PROGRESS's
\"Next:\" says otherwise. Check items off as they land.

- [ ] First item
";

const VISION_STUB: &str = "\
# Vision

The north star for this loop — what \"done\" looks like and why. Keep it short.
";

const PROGRESS_STUB: &str = "\
# Progress

Goal: {{fill in the one- or two-sentence goal}}

Next: {{the first concrete step}}

## Log
";

/// Result of scaffolding: which paths were created vs already present.
pub struct Report {
    pub created: Vec<String>,
    pub skipped: Vec<String>,
}

/// Entry point for the `init` subcommand: scaffold under the current dir, print
/// a summary, and return the process exit code.
pub fn run() -> R<i32> {
    let report = run_in(Path::new("."))?;
    for p in &report.created {
        println!("  created  {p}");
    }
    for p in &report.skipped {
        println!("  exists   {p}");
    }
    println!("\n.ralph/ ready. Fill in .ralph/PROMPT.md, then run `ralph`.");
    Ok(0)
}

/// Scaffold `.ralph/` under `root`. Idempotent — never overwrites.
pub fn run_in(root: &Path) -> R<Report> {
    let mut report = Report { created: Vec::new(), skipped: Vec::new() };
    fs::create_dir_all(root.join(".ralph/archive"))?;

    let files: [(&str, &str); 6] = [
        (".ralph/PROMPT.md", PROMPT_TEMPLATE),
        (".ralph/ralph.toml", RALPH_TOML_STUB),
        (".ralph/BACKLOG.md", BACKLOG_STUB),
        (".ralph/VISION.md", VISION_STUB),
        (".ralph/PROGRESS.md", PROGRESS_STUB),
        (".ralph/archive/.gitkeep", ""),
    ];
    for (rel, contents) in files {
        write_if_absent(root, rel, contents, &mut report)?;
    }
    ensure_gitignore(&root.join(".gitignore"), &mut report)?;
    Ok(report)
}

fn write_if_absent(root: &Path, rel: &str, contents: &str, report: &mut Report) -> R<()> {
    let path = root.join(rel);
    if path.exists() {
        report.skipped.push(rel.to_string());
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, contents)?;
        report.created.push(rel.to_string());
    }
    Ok(())
}

fn ensure_gitignore(path: &Path, report: &mut Report) -> R<()> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    if existing.contains(GITIGNORE_SENTINEL) {
        report.skipped.push(".gitignore (ralph block present)".to_string());
        return Ok(());
    }
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(GITIGNORE_BLOCK);
    fs::write(path, out)?;
    report.created.push(".gitignore (ralph block)".to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Unique temp dir per call — avoids collisions between parallel test threads
    // (process id alone is shared across threads).
    fn tmp() -> std::path::PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "ralph-init-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn scaffolds_then_is_idempotent() {
        let root = tmp();
        let r1 = run_in(&root).unwrap();
        // Everything created on the first run.
        assert!(r1.created.iter().any(|p| p == ".ralph/PROMPT.md"));
        assert!(root.join(".ralph/PROMPT.md").exists());
        assert!(root.join(".ralph/archive/.gitkeep").exists());
        assert!(root.join(".gitignore").exists());

        // Second run skips existing files and does not duplicate the block.
        let r2 = run_in(&root).unwrap();
        assert!(r2.created.is_empty(), "second init should create nothing: {:?}", r2.created);
        assert!(r2.skipped.iter().any(|p| p == ".ralph/PROMPT.md"));
        let gi = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert_eq!(gi.matches(GITIGNORE_SENTINEL).count(), 1);
    }

    #[test]
    fn appends_gitignore_block_to_existing() {
        let root = tmp();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        run_in(&root).unwrap();
        let gi = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gi.starts_with("target/\n"));
        assert!(gi.contains(GITIGNORE_SENTINEL));
    }
}
