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
# progress = \".ralph/PROGRESS.md\"
# effort = \"auto\" # haiku=low, sonnet=medium, opus=high; or set one level
# extra_args = [\"--add-dir\", \"/some/path\"]
# Self-contained prompt only: omit hooks/plugins/MCP/memory and unused tools.
# extra_args = [\"--safe-mode\", \"--tools\", \"Bash,Edit,Read,Write\"]
";

const BACKLOG_STUB: &str = "\
<!-- ralph-backlog: v1 -->
# Backlog

Ordered work list — the loop takes the first pending executable leaf. `Next:`
may refine that leaf but cannot skip it. See `ralph schema`; validate with
`ralph lint` and inspect with `ralph brief`.

- [ ] **1 — First item.**
  Describe the bounded outcome.
  Verify: {{exact command and success condition}}
";

const VISION_STUB: &str = "\
# Vision

The north star for this loop — what \"done\" looks like and why. Keep it short.
";

const PROGRESS_STUB: &str = "\
# Progress

Goal: {{fill in the one- or two-sentence goal}}

Next: 1 — {{the first concrete step within task 1}}

## Log
";

/// Result of scaffolding: which paths were created vs already present.
pub struct Report {
    pub created: Vec<String>,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
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
    for w in &report.warnings {
        eprintln!("  ⚠ {w}");
    }
    println!(
        "\n.ralph/ ready. Run `ralph schema`, fill in PROMPT.md and BACKLOG.md, then run `ralph lint`."
    );
    Ok(0)
}

/// Scaffold `.ralph/` under `root`. Idempotent — never overwrites.
pub fn run_in(root: &Path) -> R<Report> {
    let mut report = Report {
        created: Vec::new(),
        skipped: Vec::new(),
        warnings: Vec::new(),
    };
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

/// Does this .gitignore line ignore the whole `.ralph` directory (which would
/// shadow the whitelist's `!/.ralph/...` re-includes)?
fn ignores_ralph_dir(line: &str) -> bool {
    let l = line.trim();
    if l.is_empty() || l.starts_with('#') || l.starts_with('!') {
        return false;
    }
    let l = l.strip_prefix('/').unwrap_or(l);
    let l = l.strip_suffix('/').unwrap_or(l);
    l == ".ralph"
}

fn ensure_gitignore(path: &Path, report: &mut Report) -> R<()> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    if existing.contains(GITIGNORE_SENTINEL) {
        report
            .skipped
            .push(".gitignore (ralph block present)".to_string());
        return Ok(());
    }
    if existing.lines().any(ignores_ralph_dir) {
        report.warnings.push(
            "existing .gitignore ignores the whole .ralph/ directory; remove that \
             line so ralph's committed files (.ralph/PROMPT.md, ralph.toml, BACKLOG.md, …) \
             are trackable"
                .to_string(),
        );
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
        let prompt = fs::read_to_string(root.join(".ralph/PROMPT.md")).unwrap();
        assert!(prompt.contains("resolved leaf"));
        assert!(prompt.contains("RALPH_COMPLETE"));
        assert!(
            prompt.len() < 4_000,
            "template grew to {} bytes",
            prompt.len()
        );
        let backlog = fs::read_to_string(root.join(".ralph/BACKLOG.md")).unwrap();
        let parsed = crate::backlog::Document::parse(&backlog);
        assert!(parsed.schema_present);
        assert!(
            parsed.has_errors(),
            "placeholder Verify contract should require editing"
        );
        assert!(parsed
            .issues
            .iter()
            .any(|issue| issue.message.contains("non-placeholder")));

        // Second run skips existing files and does not duplicate the block.
        let r2 = run_in(&root).unwrap();
        assert!(
            r2.created.is_empty(),
            "second init should create nothing: {:?}",
            r2.created
        );
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

    #[test]
    fn warns_when_gitignore_already_ignores_ralph_dir() {
        let root = tmp();
        fs::write(root.join(".gitignore"), ".ralph/\n").unwrap();
        let r = run_in(&root).unwrap();
        assert!(r.warnings.iter().any(|w| w.contains(".ralph/")));
        // Block is still appended, effective once the user removes the shadowing line.
        let gi = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gi.contains(GITIGNORE_SENTINEL));
    }

    #[test]
    fn whitelist_tracks_config_ignores_runtime() {
        use std::process::Command;
        let root = tmp();
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["init", "-q"])
            .output()
            .unwrap();
        run_in(&root).unwrap();
        let ignored = |rel: &str| {
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["check-ignore", "-q", rel])
                .status()
                .unwrap()
                .success()
        };
        // Committed config is NOT ignored; generated runtime state IS ignored.
        assert!(!ignored(".ralph/PROMPT.md"));
        assert!(!ignored(".ralph/ralph.toml"));
        assert!(!ignored(".ralph/archive/.gitkeep"));
        assert!(ignored(".ralph/live"));
        assert!(ignored(".ralph/logs/x.log"));
    }
}
