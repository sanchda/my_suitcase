//! Resolve a bounded, schema-backed iteration brief from BACKLOG + PROGRESS.
//!
//! The backlog parser owns task selection. PROGRESS is advisory only: its first
//! `Next:` paragraph may refine the selected task, but can never select a later
//! task. This is intentionally recomputed before every fresh Claude process.

use crate::backlog::{Document, Severity};
use std::fs;
use std::path::Path;

const PROGRESS_WARN_LINES: usize = 300;
const PROGRESS_WARN_BYTES: usize = 32 * 1024;
const TASK_EXCERPT_BYTES: usize = 8 * 1024;
const PARENT_EXCERPT_BYTES: usize = 2 * 1024;
const HANDOFF_EXCERPT_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct IterationContext {
    suffix: String,
    pub target: Option<String>,
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
                    "{}: progress file is absent; no hand-off was injected",
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

    if let Some(progress) = &progress_text {
        let line_count = progress.lines().count();
        if line_count > PROGRESS_WARN_LINES || progress.len() > PROGRESS_WARN_BYTES {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                text: format!(
                    "{}: oversized progress log ({} lines, {} bytes); keep current state near the top and compact/archive history",
                    progress_path.display(),
                    line_count,
                    progress.len()
                ),
            });
        }
    }

    let selected = doc.selected_index();
    let target = selected.map(|index| {
        task_path_indices(&doc, index)
            .iter()
            .map(|task_index| doc.tasks[*task_index].id.as_str())
            .collect::<Vec<_>>()
            .join(" > ")
    });
    let suffix = match selected {
        Some(index) => build_suffix(
            backlog_path,
            progress_path,
            &doc,
            index,
            progress_text.as_deref(),
            &mut diagnostics,
        ),
        None if doc.has_errors() => invalid_suffix(backlog_path),
        None => complete_suffix(backlog_path, doc.line_count()),
    };

    IterationContext {
        suffix,
        target,
        diagnostics,
        backlog_label,
    }
}

fn build_suffix(
    backlog_path: &Path,
    progress_path: &Path,
    doc: &Document,
    selected: usize,
    progress: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) -> String {
    let task = &doc.tasks[selected];
    let path = task_path_indices(doc, selected);
    let path_label = path
        .iter()
        .map(|index| doc.tasks[*index].id.as_str())
        .collect::<Vec<_>>()
        .join(" > ");

    let handoffs = progress.map(parse_handoffs).unwrap_or_default();
    let current = handoffs.first();
    let current_key = current.and_then(|handoff| handoff.task_id.as_deref());
    let current_matches = current_key == Some(task.id.as_str());
    let chosen_handoff = if current_matches {
        current
    } else {
        if let Some(handoff) = current {
            let message = match current_key {
                Some(key) => format!(
                    "{}: line {} hand-off targets `{key}` but backlog requires `{}`; the hand-off will not override backlog order",
                    progress_path.display(),
                    handoff.line,
                    task.id
                ),
                None => format!(
                    "{}: line {} is not a canonical `Next: <task-id> — <step>` hand-off and was ignored",
                    progress_path.display(),
                    handoff.line
                ),
            };
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                text: message,
            });
        }
        None
    };

    let mut out = String::new();
    let schema_mode = if doc.schema_present {
        "v1 backlog schema"
    } else {
        "v1 compatibility mode"
    };
    out.push_str("<!-- ralph-resolved-brief: v1 -->\n");
    out.push_str("## Runner-resolved iteration brief (authoritative)\n\n");
    out.push_str(&format!(
        "Ralph parsed the complete `{}` in {schema_mode}. The executable path is **{}**; the leaf starts on line {}. Do not replace this selection with `head`, `tail`, a partial read, or a historical `Next:`.\n\n",
        backlog_path.display(),
        path_label,
        task.line
    ));

    for (position, index) in path.iter().enumerate() {
        let item = &doc.tasks[*index];
        let kind = if position + 1 == path.len() {
            "Executable leaf"
        } else {
            "Parent context"
        };
        let max_bytes = if position + 1 == path.len() {
            TASK_EXCERPT_BYTES
        } else {
            PARENT_EXCERPT_BYTES
        };
        out.push_str(&format!(
            "### {kind}: {} ({} lines {}–{})\n\n",
            item.id,
            backlog_path.display(),
            item.line,
            item.own_end_line
        ));
        out.push_str("--- BEGIN BACKLOG EXCERPT ---\n");
        out.push_str(&doc.own_excerpt(*index, max_bytes));
        out.push_str("--- END BACKLOG EXCERPT ---\n\n");
    }

    match chosen_handoff {
        Some(handoff) => {
            out.push_str(&format!(
                "### Current hand-off ({} line {})\n\n--- BEGIN HAND-OFF ---\n{}--- END HAND-OFF ---\n\n",
                progress_path.display(),
                handoff.line,
                handoff.text
            ));
        }
        None => out.push_str("No compatible `Next:` hand-off was found; derive the smallest coherent step directly from the executable leaf.\n\n"),
    }

    if let (Some(_handoff), Some(key)) = (current, current_key) {
        if key != task.id {
            out.push_str(&format!(
                "Routing conflict: the first `Next:` names `{key}` and is stale for this iteration. Ignore it; **{} remains authoritative**. Repair the first `Next:` when recording progress.\n\n",
                task.id
            ));
        }
    }

    out.push_str(
        "### Execution contract\n\n\
- Work only the executable leaf. A hand-off may refine this leaf; it cannot skip to another task.\n\
- If the leaf cannot fit one iteration, make this a `plan` pass: add ordered child stages with their own IDs and `Verify:` contracts, run `ralph lint`, and leave product code for the newly selected first stage. Do not create an unnamed slice only in PROGRESS.\n\
- Trust the excerpts above. Read only narrow referenced ranges when exact surrounding context is genuinely needed; do not dump BACKLOG or PROGRESS wholesale.\n\
- Batch independent reconnaissance. Use proportional planning effort for the task instead of open-ended analysis.\n\
- Run targeted verification while editing, then one final relevant verification after the last change. Do not rerun unchanged green commands or unrelated broad suites.\n\
- Keep the progress entry compact (roughly 12 lines), make the first canonical `Next:` point to this same task or the newly selected leaf, and compact/archive old log detail when warned.\n",
    );
    out
}

fn invalid_suffix(backlog_path: &Path) -> String {
    format!(
        "<!-- ralph-resolved-brief: v1 -->\n## Runner-resolved iteration brief\n\n`{}` is invalid or unreadable. Do not choose work heuristically; repair the backlog schema first.\n",
        backlog_path.display()
    )
}

fn complete_suffix(backlog_path: &Path, lines: usize) -> String {
    format!(
        "<!-- ralph-resolved-brief: v1 -->\n## Runner-resolved iteration brief (authoritative)\n\nRalph parsed all {lines} lines of `{}` and found no unchecked schema task. Perform only the prompt's final completion audit; do not resurrect a historical `Next:`.\n",
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

#[derive(Debug, Clone)]
struct Handoff {
    line: usize,
    text: String,
    task_id: Option<String>,
}

fn parse_handoffs(progress: &str) -> Vec<Handoff> {
    let lines: Vec<&str> = progress.lines().collect();
    let mut handoffs = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if !lines[index].starts_with("Next:") {
            index += 1;
            continue;
        }
        let start = index;
        let mut text = String::new();
        while index < lines.len()
            && (index == start
                || (!lines[index].trim().is_empty()
                    && !lines[index].starts_with("Next:")
                    && !lines[index].starts_with("## ")
                    && !lines[index].starts_with("**")))
        {
            if text.len() + lines[index].len() + 1 > HANDOFF_EXCERPT_BYTES {
                text.push_str("[… hand-off truncated by ralph …]\n");
                while index < lines.len() && !lines[index].trim().is_empty() {
                    index += 1;
                }
                break;
            }
            text.push_str(lines[index]);
            text.push('\n');
            index += 1;
        }
        let task_id = handoff_task_id(&text);
        handoffs.push(Handoff {
            line: start + 1,
            text,
            task_id,
        });
    }
    handoffs
}

fn handoff_task_id(text: &str) -> Option<String> {
    if let Some(start) = text.find("**") {
        let rest = &text[start + 2..];
        if let Some(end) = rest.find("**") {
            let label = &rest[..end];
            let candidate = label.split(" — ").next().unwrap_or(label);
            if let Some(id) = first_id_token(candidate) {
                return Some(id);
            }
        }
    }
    let rest = text.strip_prefix("Next:")?.trim_start();
    first_id_token(rest)
}

fn first_id_token(text: &str) -> Option<String> {
    let token = text
        .split_whitespace()
        .next()?
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && !matches!(ch, '.' | '_' | '-'));
    if token.is_empty()
        || !token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        || token.eq_ignore_ascii_case("backlog")
        || token.eq_ignore_ascii_case("line")
    {
        None
    } else {
        Some(token.to_string())
    }
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
    fn stale_next_cannot_override_first_unchecked_task() {
        let backlog = format!(
            "{SCHEMA_MARKER}\n- [ ] **36.8 — Current.**\n  Verify: test\n- [ ] **37.3 — Later.**\n  Verify: test\n"
        );
        let progress = "Next: BACKLOG **37.3 — stale**\n\nNext: line, prose not a hand-off\n\nNext: BACKLOG **36.8 — recovered slice**\n";
        let (backlog_path, progress_path) = tmp_files(&backlog, progress);
        let ctx = load(&backlog_path, &progress_path);
        assert_eq!(ctx.target.as_deref(), Some("36.8"));
        assert!(!ctx.render().contains("recovered slice"));
        assert!(ctx.render().contains("No compatible `Next:`"));
        assert!(ctx.render().contains("Routing conflict"));
        assert!(ctx
            .warnings()
            .any(|warning| warning.contains("will not override backlog order")));
    }

    #[test]
    fn current_matching_next_is_used() {
        let backlog = format!("{SCHEMA_MARKER}\n- [ ] **2 — Work.**\n  Verify: test\n");
        let (backlog_path, progress_path) = tmp_files(&backlog, "Next: 2 — implement parser\n");
        let ctx = load(&backlog_path, &progress_path);
        assert!(!ctx.has_errors());
        assert!(!ctx.is_complete());
        assert!(ctx.render().contains("implement parser"));
    }

    #[test]
    fn noncanonical_next_prose_is_ignored() {
        let backlog = format!("{SCHEMA_MARKER}\n- [ ] **2 — Work.**\n  Verify: test\n");
        let progress = "Next: line, this is historical prose\n\nNext: 2 — real hand-off\n";
        let (backlog_path, progress_path) = tmp_files(&backlog, progress);
        let ctx = load(&backlog_path, &progress_path);
        assert!(!ctx.render().contains("real hand-off"));
        assert!(ctx.render().contains("No compatible `Next:`"));
        assert!(!ctx.render().contains("this is historical prose"));
        assert!(ctx
            .warnings()
            .any(|warning| warning.contains("not a canonical")));
    }

    #[test]
    fn composed_prompt_preserves_stable_base_first() {
        let backlog = format!("{SCHEMA_MARKER}\n- [ ] **2 — Work.**\n  Verify: test\n");
        let (backlog_path, progress_path) = tmp_files(&backlog, "Next: 2 — do it\n");
        let ctx = load(&backlog_path, &progress_path);
        let prompt = ctx.compose("stable base\n");
        assert!(prompt.starts_with("stable base\n\n<!-- ralph-resolved-brief"));
    }

    #[test]
    fn all_complete_ignores_historical_next() {
        let backlog = format!("{SCHEMA_MARKER}\n- [x] **1 — Done.** Verify: test\n");
        let (backlog_path, progress_path) = tmp_files(&backlog, "Next: 1 — old\n");
        let ctx = load(&backlog_path, &progress_path);
        assert_eq!(ctx.target, None);
        assert!(ctx.is_complete());
        assert!(ctx.render().contains("found no unchecked schema task"));
        assert!(!ctx.render().contains("Next: 1"));
    }

    #[test]
    fn invalid_backlog_is_fatal_context() {
        let (backlog_path, progress_path) = tmp_files("- [ ] not schema\n", "");
        let ctx = load(&backlog_path, &progress_path);
        assert!(ctx.has_errors());
        assert!(!ctx.is_complete());
        assert!(ctx.render().contains("repair the backlog schema"));
    }
}
