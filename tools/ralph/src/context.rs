//! Resolve a bounded, schema-backed iteration brief from BACKLOG + PROGRESS.
//!
//! The backlog parser owns task selection. PROGRESS is a plain carry-forward
//! note the runner writes and we inject verbatim (no id parsing): it may clarify
//! the resolved leaf but can never reroute to a later task. This is intentionally
//! recomputed before every fresh Claude process.

use crate::backlog::{Document, Severity};
use std::fs;
use std::path::Path;

const TASK_EXCERPT_BYTES: usize = 8 * 1024;
const PARENT_EXCERPT_BYTES: usize = 2 * 1024;
const ALL_PARENTS_EXCERPT_BYTES: usize = 4 * 1024;
const RUNNER_CONTRACT: &str = "\
<!-- ralph-runner-contract: v1 -->
When a pending leaf exists, work only that leaf; a carry-forward note may clarify it, never reroute. If the leaf is too large, make a plan pass: add ordered child stages with IDs and `Verify:` contracts, run `ralph lint`, and leave product code for the selected child. Trust the excerpts; use only narrow file reads. Verify proportionally. Do not write PROGRESS or a `Next:` line; the runner owns the hand-off.
";

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct IterationContext {
    suffix: String,
    pub target: Option<String>,
    /// The selected leaf's title (the bolded one-liner), for human status lines.
    pub target_title: Option<String>,
    /// The resolved leaf's own `(tier/…)` model decoration, if any.
    pub model_hint: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
    backlog_label: String,
}

impl IterationContext {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|item| item.severity == Severity::Error)
    }

    pub fn is_complete(&self) -> bool {
        !self.has_errors() && self.target.is_none()
    }

    pub fn errors(&self) -> impl Iterator<Item = &str> {
        self.diagnostics
            .iter()
            .filter(|item| item.severity == Severity::Error)
            .map(|item| item.text.as_str())
    }

    pub fn warnings(&self) -> impl Iterator<Item = &str> {
        self.diagnostics
            .iter()
            .filter(|item| item.severity == Severity::Warning)
            .map(|item| item.text.as_str())
    }

    pub fn compose(&self, base_prompt: &str) -> String {
        let mut prompt = base_prompt.trim_end().to_string();
        prompt.push_str("\n\n");
        prompt.push_str(RUNNER_CONTRACT);
        prompt.push('\n');
        prompt.push_str(&self.suffix);
        if !prompt.ends_with('\n') {
            prompt.push('\n');
        }
        prompt
    }

    /// Human-facing output for `ralph brief`.
    pub fn render(&self) -> String {
        let mut out = self.lint_report();
        out.push('\n');
        out.push_str(RUNNER_CONTRACT);
        out.push('\n');
        out.push_str(&self.suffix);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }

    pub fn lint_report(&self) -> String {
        let mut out = format!("Backlog lint: {}\n", self.backlog_label);
        if self.diagnostics.is_empty() {
            out.push_str("ok: schema is valid\n");
        } else {
            for item in &self.diagnostics {
                let kind = match item.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                };
                out.push_str(&format!("{kind}: {}\n", item.text));
            }
        }
        if let Some(target) = &self.target {
            out.push_str(&format!("selected: {target}\n"));
        } else if !self.has_errors() {
            out.push_str("selected: none (all schema tasks are complete)\n");
        }
        out
    }
}

/// Load and resolve the two driving files. BACKLOG is required; PROGRESS is
/// optional but strongly recommended.
pub fn load(backlog_path: &Path, progress_path: &Path) -> IterationContext {
    let backlog_label = backlog_path.display().to_string();
    let mut diagnostics = Vec::new();
    let backlog_text = match fs::read_to_string(backlog_path) {
        Ok(text) => text,
        Err(error) => {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                text: format!("{}: cannot read backlog: {error}", backlog_path.display()),
            });
            return IterationContext {
                suffix: invalid_suffix(backlog_path),
                target: None,
                target_title: None,
                model_hint: None,
                diagnostics,
                backlog_label,
            };
        }
    };

    let doc = Document::parse(&backlog_text);
    diagnostics.extend(doc.issues.iter().map(|issue| Diagnostic {
        severity: issue.severity,
        text: if issue.line == 0 {
            format!("{}: {}", backlog_path.display(), issue.message)
        } else {
            format!(
                "{}:{}: {}",
                backlog_path.display(),
                issue.line,
                issue.message
            )
        },
    }));

    let progress_text = match fs::read_to_string(progress_path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                text: format!(
                    "{}: progress file is absent; no carry-forward was injected",
                    progress_path.display()
                ),
            });
            None
        }
        Err(error) => {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                text: format!("{}: cannot read progress: {error}", progress_path.display()),
            });
            None
        }
    };

    let selected = doc.selected_index();
    let target = selected.map(|index| {
        task_path_indices(&doc, index)
            .iter()
            .map(|task_index| doc.tasks[*task_index].id.as_str())
            .collect::<Vec<_>>()
            .join(" > ")
    });
    let target_title = selected.map(|index| doc.tasks[index].title.clone());
    let suffix = match selected {
        Some(index) => build_suffix(backlog_path, &doc, index, progress_text.as_deref()),
        None if doc.has_errors() => invalid_suffix(backlog_path),
        None => complete_suffix(backlog_path, doc.line_count()),
    };

    IterationContext {
        suffix,
        target,
        target_title,
        model_hint: selected.and_then(|index| doc.model_hint(index)),
        diagnostics,
        backlog_label,
    }
}

fn build_suffix(
    backlog_path: &Path,
    doc: &Document,
    selected: usize,
    progress: Option<&str>,
) -> String {
    let task = &doc.tasks[selected];
    let path = task_path_indices(doc, selected);
    let path_label = path
        .iter()
        .map(|index| doc.tasks[*index].id.as_str())
        .collect::<Vec<_>>()
        .join(" > ");

    let mut out = String::new();
    let schema_mode = if doc.schema_present {
        "v1 backlog schema"
    } else {
        "v1 compatibility mode"
    };
    out.push_str("<!-- ralph-resolved-brief: v1 -->\n");
    out.push_str("## Resolved target (authoritative)\n\n");
    out.push_str(&format!(
        "**{}** at `{}:{}` ({schema_mode}; full backlog parsed).\n\n",
        path_label,
        backlog_path.display(),
        task.line
    ));

    let parent_count = path.len().saturating_sub(1);
    let parent_excerpt_bytes = ALL_PARENTS_EXCERPT_BYTES
        .checked_div(parent_count)
        .unwrap_or(0)
        .min(PARENT_EXCERPT_BYTES);
    for (position, index) in path.iter().enumerate() {
        let item = &doc.tasks[*index];
        let kind = if position + 1 == path.len() {
            "Leaf"
        } else {
            "Parent"
        };
        let max_bytes = if position + 1 == path.len() {
            TASK_EXCERPT_BYTES
        } else {
            parent_excerpt_bytes
        };
        out.push_str(&format!(
            "### {kind} {} (lines {}–{})\n\n",
            item.id, item.line, item.own_end_line
        ));
        out.push_str(&format!("--- BEGIN {} ---\n", item.id));
        out.push_str(&doc.own_excerpt(*index, max_bytes));
        out.push_str(&format!("--- END {} ---\n\n", item.id));
    }

    match progress {
        Some(text) if !text.trim().is_empty() => {
            out.push_str("### Carry-forward (from the previous iteration)\n\n");
            out.push_str("--- BEGIN CARRY-FORWARD ---\n");
            out.push_str(text.trim_end());
            out.push_str("\n--- END CARRY-FORWARD ---\n\n");
        }
        _ => out.push_str("Carry-forward: none; use the leaf directly.\n\n"),
    }
    out
}

fn invalid_suffix(backlog_path: &Path) -> String {
    format!(
        "<!-- ralph-resolved-brief: v1 -->\n## Invalid backlog\n\n`{}` is invalid or unreadable. Repair the backlog schema; do not choose work heuristically.\n",
        backlog_path.display()
    )
}

fn complete_suffix(backlog_path: &Path, lines: usize) -> String {
    format!(
        "<!-- ralph-resolved-brief: v1 -->\n## Backlog complete\n\nAll {lines} lines of `{}` were parsed; no pending task remains. Perform only the final completion audit.\n",
        backlog_path.display()
    )
}

fn task_path_indices(doc: &Document, selected: usize) -> Vec<usize> {
    let mut path = vec![selected];
    let mut parent = doc.tasks[selected].parent;
    while let Some(index) = parent {
        path.push(index);
        parent = doc.tasks[index].parent;
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backlog::SCHEMA_MARKER;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmp_files(backlog: &str, progress: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "ralph-context-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let backlog_path = root.join("BACKLOG.md");
        let progress_path = root.join("PROGRESS.md");
        fs::write(&backlog_path, backlog).unwrap();
        fs::write(&progress_path, progress).unwrap();
        (backlog_path, progress_path)
    }

    #[test]
    fn carry_forward_is_injected_verbatim_without_warnings() {
        let backlog = format!("{SCHEMA_MARKER}\n# Backlog\n\n- [ ] **1 — Do it.** Verify: yes\n");
        let (backlog_path, progress_path) = tmp_files(
            &backlog,
            "- watch the null-guard in foo.rs\n- 2 is coprime\n",
        );
        let ctx = load(&backlog_path, &progress_path);
        let brief = ctx.render();
        assert!(brief.contains("Carry-forward"));
        assert!(brief.contains("watch the null-guard in foo.rs"));
        assert_eq!(ctx.warnings().count(), 0);
    }

    #[test]
    fn absent_carry_forward_reads_none() {
        let backlog = format!("{SCHEMA_MARKER}\n# Backlog\n\n- [ ] **1 — Do it.** Verify: yes\n");
        let root = std::env::temp_dir().join(format!("ralph-ctx-none-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let backlog_path = root.join("BACKLOG.md");
        fs::write(&backlog_path, &backlog).unwrap();
        let progress_path = root.join("PROGRESS.md"); // does not exist
        let ctx = load(&backlog_path, &progress_path);
        assert!(ctx.render().contains("Carry-forward: none"));
    }

    #[test]
    fn carry_forward_note_clarifies_the_leaf() {
        let backlog = format!("{SCHEMA_MARKER}\n- [ ] **2 — Work.**\n  Verify: test\n");
        let (backlog_path, progress_path) =
            tmp_files(&backlog, "- leaf 2 still needs the parser wired up\n");
        let ctx = load(&backlog_path, &progress_path);
        assert!(!ctx.has_errors());
        assert!(!ctx.is_complete());
        assert!(ctx.render().contains("needs the parser wired up"));
    }

    #[test]
    fn composed_prompt_preserves_stable_base_first() {
        let backlog = format!("{SCHEMA_MARKER}\n- [ ] **2 — Work.**\n  Verify: test\n");
        let (backlog_path, progress_path) = tmp_files(&backlog, "- do it\n");
        let ctx = load(&backlog_path, &progress_path);
        let prompt = ctx.compose("stable base\n");
        assert!(prompt.starts_with("stable base\n\n<!-- ralph-runner-contract"));
        assert!(
            prompt.find("ralph-runner-contract").unwrap() < prompt.find("Resolved target").unwrap()
        );
    }

    #[test]
    fn target_title_is_the_selected_leaf_title() {
        let backlog = format!("{SCHEMA_MARKER}\n- [ ] **37.2 — Deep Field corridor.** Verify: test\n");
        let (backlog_path, progress_path) = tmp_files(&backlog, "");
        let ctx = load(&backlog_path, &progress_path);
        assert_eq!(ctx.target.as_deref(), Some("37.2"));
        assert_eq!(ctx.target_title.as_deref(), Some("Deep Field corridor."));
    }

    #[test]
    fn complete_backlog_has_no_target_title() {
        let backlog = format!("{SCHEMA_MARKER}\n- [x] **1 — Done.** Verify: test\n");
        let (backlog_path, progress_path) = tmp_files(&backlog, "");
        let ctx = load(&backlog_path, &progress_path);
        assert_eq!(ctx.target_title, None);
    }

    #[test]
    fn all_complete_ignores_historical_next() {
        let backlog = format!("{SCHEMA_MARKER}\n- [x] **1 — Done.** Verify: test\n");
        let (backlog_path, progress_path) = tmp_files(&backlog, "Next: 1 — old\n");
        let ctx = load(&backlog_path, &progress_path);
        assert_eq!(ctx.target, None);
        assert!(ctx.is_complete());
        assert!(ctx.render().contains("no pending task remains"));
        assert!(!ctx.render().contains("Next: 1"));
    }

    #[test]
    fn invalid_backlog_is_fatal_context() {
        let (backlog_path, progress_path) = tmp_files("- [ ] not schema\n", "");
        let ctx = load(&backlog_path, &progress_path);
        assert!(ctx.has_errors());
        assert!(!ctx.is_complete());
        assert!(ctx.render().contains("Repair the backlog schema"));
    }

    #[test]
    fn deep_parent_chain_shares_one_excerpt_budget() {
        let mut backlog = format!("{SCHEMA_MARKER}\n");
        for depth in 1..=32 {
            let indent = "  ".repeat(depth - 1);
            let id = (1..=depth)
                .map(|part| part.to_string())
                .collect::<Vec<_>>()
                .join(".");
            backlog.push_str(&format!(
                "{indent}- [ ] **{id} — Stage {depth}.** {}\n{indent}  Verify: test\n",
                "parent detail ".repeat(40)
            ));
        }
        let (backlog_path, progress_path) = tmp_files(&backlog, "");
        let rendered = load(&backlog_path, &progress_path).render();
        assert!(rendered.contains("### Parent 1"));
        assert!(rendered.contains("### Leaf 1.2.3.4.5"));
        assert!(
            rendered.len() < 16_000,
            "deep brief was {} bytes",
            rendered.len()
        );
    }
}
