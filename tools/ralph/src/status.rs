//! `ralph status [--json]` — a machine- and human-readable snapshot of the loop's
//! backlog frontier. Read-only; run-state (is a loop alive?) is the caller's job.

use crate::backlog::Document;
use crate::config::Config;
use crate::R;
use serde::Serialize;

const EXCERPT_BYTES: usize = 1200;
const UPCOMING_WINDOW: usize = 4;

/// Read the iteration counter without creating the runtime dir (unlike
/// `State::open`), keeping `status` read-only.
fn read_iteration(dir: &std::path::Path) -> u64 {
    std::fs::read_to_string(dir.join("iteration"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

#[derive(Debug, Serialize, PartialEq)]
pub struct CurrentTask {
    pub id: String,
    pub label: String,
    pub excerpt: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Report {
    pub iteration: u64,
    pub pending_leaf_count: usize,
    pub current: Option<CurrentTask>,
    pub upcoming: Vec<String>,
}

/// Build the snapshot from a parsed backlog and the current iteration counter.
fn build_report(doc: &Document, iteration: u64) -> Report {
    let selected = doc.selected_index();
    let current = selected.map(|index| {
        let task = &doc.tasks[index];
        CurrentTask {
            id: task.id.clone(),
            label: format!("{} — {}", task.id, task.title),
            excerpt: doc.own_excerpt(index, EXCERPT_BYTES),
        }
    });
    // `upcoming_leaf_labels` includes the selected leaf first; drop it so
    // `upcoming` is strictly the tasks after the current one.
    let mut upcoming = doc.upcoming_leaf_labels(UPCOMING_WINDOW);
    if !upcoming.is_empty() {
        upcoming.remove(0);
    }
    Report {
        iteration,
        pending_leaf_count: doc.pending_leaf_count(),
        current,
        upcoming,
    }
}

/// `ralph status [--json]`. Resolves driving-file paths via `load_base`, reads
/// the backlog + iteration counter, and prints a snapshot.
pub fn run(args: &[String]) -> R<i32> {
    let json = args.iter().any(|a| a == "--json");
    // Strip our own flag before config resolution so path lookup is unaffected.
    let rest: Vec<String> = args
        .iter()
        .filter(|a| a.as_str() != "--json")
        .cloned()
        .collect();
    let cfg: Config = crate::config::load_base(&rest)?;
    let iteration = read_iteration(&cfg.dir);
    let text = std::fs::read_to_string(&cfg.backlog)
        .map_err(|e| format!("{}: cannot read backlog: {e}", cfg.backlog.display()))?;
    let doc = Document::parse(&text);
    if doc.has_errors() {
        return Err("backlog schema is invalid; run `ralph lint` for details".into());
    }
    let report = build_report(&doc, iteration);
    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        match &report.current {
            Some(c) => println!(
                "iter {} · {} pending · current: {}",
                report.iteration, report.pending_leaf_count, c.label
            ),
            None => println!("iter {} · backlog complete", report.iteration),
        }
        for label in &report.upcoming {
            println!("  next: {label}");
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backlog::SCHEMA_MARKER;

    #[test]
    fn report_reflects_selected_leaf_and_upcoming() {
        let text = format!(
            "{SCHEMA_MARKER}\n# B\n- [x] **1 — Done.** Verify: y\n- [ ] **2 — Current.**\n  Verify: cargo test\n- [ ] **3 — Next.** Verify: y\n- [ ] **4 — After.** Verify: y\n"
        );
        let doc = Document::parse(&text);
        let report = build_report(&doc, 7);
        assert_eq!(report.iteration, 7);
        assert_eq!(report.pending_leaf_count, 3);
        let current = report.current.expect("a selected leaf");
        assert_eq!(current.id, "2");
        assert_eq!(current.label, "2 — Current.");
        assert!(current.excerpt.contains("cargo test"));
        assert_eq!(
            report.upcoming,
            vec!["3 — Next.".to_string(), "4 — After.".to_string()]
        );
    }

    #[test]
    fn report_is_empty_when_complete() {
        let doc = Document::parse(&format!("{SCHEMA_MARKER}\n- [x] **1 — Done.** Verify: y\n"));
        let report = build_report(&doc, 3);
        assert_eq!(report.pending_leaf_count, 0);
        assert!(report.current.is_none());
        assert!(report.upcoming.is_empty());
    }
}
