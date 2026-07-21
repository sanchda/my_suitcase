//! Opportunistic BACKLOG curation: lift the fully-completed leading sections out
//! of the live backlog into `archive/BACKLOG-completed.md`, keeping the live file
//! scoped to pending work. Safe because the v1 backlog has no id cross-references
//! (selection is pure document order), so a prefix lift preserves every
//! invariant. Best-effort: any failure is a no-op that leaves the backlog intact.

use crate::backlog::Document;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

/// Sweep completed leading sections. Returns the number of sections swept.
/// `archive_dir` receives an appended `BACKLOG-completed.md`.
pub fn sweep(backlog_path: &Path, archive_dir: &Path) -> usize {
    let Ok(text) = fs::read_to_string(backlog_path) else {
        return 0;
    };
    let doc = Document::parse(&text);
    if doc.has_errors() {
        return 0; // never mutate an invalid backlog
    }
    let Some(span) = doc.completed_leading_prefix() else {
        return 0;
    };
    let lines: Vec<&str> = text.lines().collect();
    // 1-based [first_line, last_line] inclusive → 0-based slice.
    let start = span.first_line - 1;
    let end = span.last_line; // exclusive upper bound for the swept block
    if start >= lines.len() || end > lines.len() || start >= end {
        return 0;
    }
    let swept: String = {
        let mut s = lines[start..end].join("\n");
        s.push('\n');
        s
    };
    let remaining: String = {
        let mut keep: Vec<&str> = Vec::new();
        keep.extend_from_slice(&lines[..start]); // header/preamble
        keep.extend_from_slice(&lines[end..]); // frontier section onward
        let mut s = keep.join("\n");
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s
    };

    // Never write an invalid backlog: if the lift would leave a schema-invalid
    // remainder (e.g. all sections complete → header-only, no tasks), no-op.
    // Validate before touching the archive so we never append-then-bail.
    if Document::parse(&remaining).has_errors() {
        return 0;
    }

    if fs::create_dir_all(archive_dir).is_err() {
        return 0;
    }
    let archive = archive_dir.join("BACKLOG-completed.md");
    // Only treat a missing file as an empty archive; any other read error (perms,
    // transient IO, non-UTF8) means an existing archive we must NOT clobber → bail.
    let mut acc = match fs::read_to_string(&archive) {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
        Err(_) => return 0,
    };
    if !acc.is_empty() && !acc.ends_with('\n') {
        acc.push('\n');
    }
    acc.push_str(&swept);
    // Archive before pruning: on a partial failure a re-sweep may duplicate an
    // archive entry, which is preferable to pruning the backlog before the work is
    // safely archived. Do not reorder these two writes.
    if fs::write(&archive, acc).is_err() {
        return 0;
    }
    // Note: reconstruction via `text.lines()`/`join("\n")` normalizes CRLF → LF.
    if fs::write(backlog_path, remaining).is_err() {
        return 0;
    }
    span.section_count
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmp() -> std::path::PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "ralph-curate-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn sweeps_completed_prefix_and_preserves_frontier() {
        let root = tmp();
        let backlog = root.join("BACKLOG.md");
        let marker = crate::backlog::SCHEMA_MARKER;
        fs::write(
            &backlog,
            format!(
                "{marker}\n# Backlog\n\nIntro prose.\n\n\
                 - [x] **1 — Done.** Verify: yes\n\
                 - [x] **2 — Done.** Verify: yes\n\
                 - [ ] **3 — Pending.** Verify: yes\n"
            ),
        )
        .unwrap();
        let swept = sweep(&backlog, &root.join("archive"));
        assert_eq!(swept, 2);

        let live = fs::read_to_string(&backlog).unwrap();
        assert!(live.contains("Intro prose."), "header preserved");
        assert!(live.contains("**3 — Pending.**"), "frontier preserved");
        assert!(!live.contains("**1 — Done.**"), "section 1 lifted out");

        let archived = fs::read_to_string(root.join("archive/BACKLOG-completed.md")).unwrap();
        assert!(archived.contains("**1 — Done.**"));
        assert!(archived.contains("**2 — Done.**"));

        // The pruned live backlog still parses and selects task 3.
        let doc = Document::parse(&live);
        assert!(!doc.has_errors());
        assert_eq!(doc.tasks[doc.selected_index().unwrap()].id, "3");

        // Idempotent: nothing left to sweep.
        assert_eq!(sweep(&backlog, &root.join("archive")), 0);
    }

    #[test]
    fn appends_to_existing_archive_in_order() {
        // Accumulation path: an archive with prior swept content gets the new
        // sections appended after it (old before new), not overwritten.
        let root = tmp();
        let archive_dir = root.join("archive");
        fs::create_dir_all(&archive_dir).unwrap();
        let archive = archive_dir.join("BACKLOG-completed.md");
        fs::write(&archive, "- [x] **0 — Earlier.** Verify: yes\n").unwrap();

        let backlog = root.join("BACKLOG.md");
        let marker = crate::backlog::SCHEMA_MARKER;
        fs::write(
            &backlog,
            format!(
                "{marker}\n# Backlog\n\n\
                 - [x] **1 — Done.** Verify: yes\n\
                 - [ ] **2 — Pending.** Verify: yes\n"
            ),
        )
        .unwrap();

        assert_eq!(sweep(&backlog, &archive_dir), 1);

        let archived = fs::read_to_string(&archive).unwrap();
        assert!(
            archived.contains("**0 — Earlier.**"),
            "pre-seeded content kept"
        );
        assert!(
            archived.contains("**1 — Done.**"),
            "newly swept section present"
        );
        let earlier = archived.find("**0 — Earlier.**").unwrap();
        let done = archived.find("**1 — Done.**").unwrap();
        assert!(earlier < done, "old entry precedes new entry");
    }

    #[test]
    fn noop_when_all_sections_complete() {
        // No pending frontier to preserve → curation does nothing; terminal
        // archival (loop-wiring) takes over. Must not leave a header-only backlog.
        let root = tmp();
        let backlog = root.join("BACKLOG.md");
        let marker = crate::backlog::SCHEMA_MARKER;
        let original = format!(
            "{marker}\n# Backlog\n\nIntro.\n\n\
             - [x] **1 — Done.** Verify: yes\n\
             - [x] **2 — Done.** Verify: yes\n"
        );
        fs::write(&backlog, &original).unwrap();
        assert_eq!(sweep(&backlog, &root.join("archive")), 0);

        // Backlog on disk is untouched and still holds both sections.
        let live = fs::read_to_string(&backlog).unwrap();
        assert_eq!(live, original, "backlog unchanged");
        assert!(live.contains("**1 — Done.**"));
        assert!(live.contains("**2 — Done.**"));
        assert!(!root.join("archive/BACKLOG-completed.md").exists());
    }

    #[test]
    fn noop_when_nothing_complete() {
        let root = tmp();
        let backlog = root.join("BACKLOG.md");
        let marker = crate::backlog::SCHEMA_MARKER;
        fs::write(
            &backlog,
            format!("{marker}\n# Backlog\n\n- [ ] **1 — Pending.** Verify: yes\n"),
        )
        .unwrap();
        assert_eq!(sweep(&backlog, &root.join("archive")), 0);
        assert!(!root.join("archive/BACKLOG-completed.md").exists());
    }
}
