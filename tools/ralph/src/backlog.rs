//! The deliberately small Markdown schema used by Ralph backlogs.
//!
//! Executable work is a checkbox whose bold label is `<id> — <title>`.
//! Two-space-indented child checkboxes are ordered stages. The first unchecked
//! task with no unchecked descendants is the next executable leaf.

use std::collections::{HashMap, HashSet};

pub const SCHEMA_MARKER: &str = "<!-- ralph-backlog: v1 -->";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    pub line: usize,
    pub message: String,
}

impl Issue {
    fn error(line: usize, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            line,
            message: message.into(),
        }
    }

    fn warning(line: usize, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            line,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub checked: bool,
    pub indent: usize,
    pub line: usize,
    pub end_line: usize,
    pub own_end_line: usize,
    pub parent: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Document {
    lines: Vec<String>,
    pub tasks: Vec<Task>,
    pub issues: Vec<Issue>,
    pub schema_present: bool,
}

impl Document {
    pub fn parse(text: &str) -> Self {
        let lines: Vec<String> = text.lines().map(String::from).collect();
        let mut schema_present = false;
        let mut issues = Vec::new();

        let mut tasks: Vec<Task> = Vec::new();
        let mut ids: HashMap<String, usize> = HashMap::new();
        let mut fence: Option<(char, usize)> = None;
        let mut headings = Vec::new();
        for (offset, line) in lines.iter().enumerate() {
            let line_no = offset + 1;
            let trimmed = line.trim_start();
            if let Some((delimiter, _)) = fence {
                let closes = if delimiter == '`' {
                    trimmed.starts_with("```")
                } else {
                    trimmed.starts_with("~~~")
                };
                if closes {
                    fence = None;
                }
                continue;
            }
            if trimmed.starts_with("```") {
                fence = Some(('`', line_no));
                continue;
            }
            if trimmed.starts_with("~~~") {
                fence = Some(('~', line_no));
                continue;
            }
            if trimmed == SCHEMA_MARKER {
                schema_present = true;
                continue;
            }
            if trimmed.starts_with("<!-- ralph-backlog:") {
                issues.push(Issue::error(
                    line_no,
                    format!(
                        "unsupported backlog schema marker `{trimmed}`; expected `{SCHEMA_MARKER}`"
                    ),
                ));
                continue;
            }
            if line.starts_with('#') {
                headings.push(line_no);
            }
            let header = match parse_task_line(line) {
                Ok(Some(header)) => header,
                Ok(None) => continue,
                Err(message) => {
                    issues.push(Issue::error(line_no, message));
                    continue;
                }
            };

            if header.indent % 2 != 0 {
                issues.push(Issue::error(
                    line_no,
                    "task indentation must use exactly two spaces per stage level",
                ));
            }
            if let Some(previous_line) = ids.insert(header.id.clone(), line_no) {
                issues.push(Issue::error(
                    line_no,
                    format!(
                        "duplicate task id `{}` (first used on line {previous_line})",
                        header.id
                    ),
                ));
            }

            let parent = if header.indent == 0 {
                None
            } else {
                let found = tasks
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, task)| task.indent < header.indent);
                match found {
                    Some((index, task)) => {
                        if task.indent + 2 != header.indent {
                            issues.push(Issue::error(
                                line_no,
                                "stage nesting skipped a level; indent two spaces below its parent",
                            ));
                        }
                        Some(index)
                    }
                    None => {
                        issues.push(Issue::error(line_no, "indented stage has no parent task"));
                        None
                    }
                }
            };

            if let Some(parent_index) = parent {
                let parent_task = &tasks[parent_index];
                let prefix = format!("{}.", parent_task.id);
                if !header.id.starts_with(&prefix) {
                    issues.push(Issue::error(
                        line_no,
                        format!(
                            "stage id `{}` must begin with parent prefix `{prefix}`",
                            header.id
                        ),
                    ));
                }
                if parent_task.checked && !header.checked {
                    issues.push(Issue::error(
                        line_no,
                        format!(
                            "checked parent `{}` contains an unchecked stage",
                            parent_task.id
                        ),
                    ));
                }
            }

            tasks.push(Task {
                id: header.id,
                title: header.title,
                checked: header.checked,
                indent: header.indent,
                line: line_no,
                end_line: lines.len(),
                own_end_line: lines.len(),
                parent,
            });
        }

        if let Some((_, start_line)) = fence {
            issues.push(Issue::error(
                start_line,
                "unclosed fenced code block can hide backlog tasks",
            ));
        }

        if !schema_present {
            issues.insert(
                0,
                Issue::warning(
                    1,
                    format!(
                        "missing schema marker `{SCHEMA_MARKER}`; parsed as v1 compatibility mode"
                    ),
                ),
            );
        }

        for index in 0..tasks.len() {
            let task_end = tasks
                .iter()
                .skip(index + 1)
                .find(|next| next.indent <= tasks[index].indent)
                .map(|next| next.line.saturating_sub(1))
                .unwrap_or(lines.len());
            let heading_end = headings
                .iter()
                .copied()
                .find(|line| *line > tasks[index].line)
                .map(|line| line.saturating_sub(1))
                .unwrap_or(lines.len());
            let end_line = task_end.min(heading_end);
            let own_end_line = tasks
                .get(index + 1)
                .filter(|next| next.indent > tasks[index].indent)
                .map(|next| next.line.saturating_sub(1))
                .unwrap_or(end_line);
            tasks[index].end_line = end_line;
            tasks[index].own_end_line = own_end_line.min(end_line);
        }

        if tasks.is_empty() {
            issues.push(Issue::error(0, "backlog contains no schema tasks"));
        }

        let missing_verify: Vec<&Task> = tasks
            .iter()
            .filter(|task| !task.checked && !has_valid_verify(&lines, task))
            .collect();
        if schema_present {
            for task in &missing_verify {
                issues.push(Issue::error(
                    task.line,
                    format!(
                        "pending task `{}` needs a non-placeholder `Verify:` contract before any child stage",
                        task.id
                    ),
                ));
            }
        } else if let Some(first) = missing_verify.first() {
            issues.push(Issue::warning(
                first.line,
                format!(
                    "{} pending task(s) lack a non-placeholder `Verify:` contract; compatibility mode permits this, but v1 strict mode will reject it",
                    missing_verify.len()
                ),
            ));
        }

        let mut first_pending: HashMap<Option<usize>, &Task> = HashMap::new();
        let mut warned_groups: HashSet<Option<usize>> = HashSet::new();
        for task in &tasks {
            if !task.checked {
                first_pending.entry(task.parent).or_insert(task);
            } else if let Some(pending) = first_pending.get(&task.parent) {
                if warned_groups.insert(task.parent) {
                    issues.push(Issue::warning(
                        task.line,
                        format!(
                            "checked task `{}` appears after pending sibling `{}`; document order was bypassed (routing still selects the first pending sibling)",
                            task.id, pending.id
                        ),
                    ));
                }
            }
        }

        Self {
            lines,
            tasks,
            issues,
            schema_present,
        }
    }

    pub fn has_errors(&self) -> bool {
        self.issues
            .iter()
            .any(|issue| issue.severity == Severity::Error)
    }

    /// First pending executable leaf. A parent with pending children is a
    /// container; once all children are checked, the parent becomes its final
    /// verification/closure step.
    pub fn selected_index(&self) -> Option<usize> {
        self.tasks
            .iter()
            .enumerate()
            .find(|(index, task)| !task.checked && !has_unchecked_descendant(&self.tasks, *index))
            .map(|(index, _)| index)
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    #[cfg(test)]
    pub fn selected_path(&self, index: usize) -> Vec<&Task> {
        let mut path = vec![&self.tasks[index]];
        let mut parent = self.tasks[index].parent;
        while let Some(parent_index) = parent {
            path.push(&self.tasks[parent_index]);
            parent = self.tasks[parent_index].parent;
        }
        path.reverse();
        path
    }

    /// The task's own prose, excluding child stages, with a hard byte bound.
    pub fn own_excerpt(&self, index: usize, max_bytes: usize) -> String {
        let task = &self.tasks[index];
        bounded_lines(
            &self.lines,
            task.line.saturating_sub(1),
            task.own_end_line,
            max_bytes,
        )
    }
}

struct Header {
    id: String,
    title: String,
    checked: bool,
    indent: usize,
}

fn parse_task_line(line: &str) -> Result<Option<Header>, String> {
    let trimmed = line.trim_start();
    let alternate_checkbox = ["* [ ] ", "* [x] ", "* [X] ", "+ [ ] ", "+ [x] ", "+ [X] "]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
        || matches!(
            trimmed,
            "* [ ]" | "* [x]" | "* [X]" | "+ [ ]" | "+ [x]" | "+ [X]"
        );
    if alternate_checkbox {
        return Err("schema task checkboxes must use the `-` bullet".into());
    }
    let recognized_checkbox = ["- [ ] ", "- [x] ", "- [X] "]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
        || matches!(trimmed, "- [ ]" | "- [x]" | "- [X]");
    let looks_like_malformed_task = trimmed.starts_with("- [") && trimmed.contains("**");
    if !recognized_checkbox && !looks_like_malformed_task {
        return Ok(None);
    }
    let prefix_len = line.len() - trimmed.len();
    let whitespace = &line[..prefix_len];
    if whitespace.contains('\t') {
        return Err("task indentation must use spaces, not tabs".into());
    }

    let (checked, rest) = if let Some(rest) = trimmed.strip_prefix("- [ ] ") {
        (false, rest)
    } else if let Some(rest) = trimmed.strip_prefix("- [x] ") {
        (true, rest)
    } else if let Some(rest) = trimmed.strip_prefix("- [X] ") {
        (true, rest)
    } else {
        return Err("task checkbox must be `- [ ] ` or `- [x] `".into());
    };

    let label = rest
        .strip_prefix("**")
        .ok_or_else(|| "task label must be bold: `**<id> — <title>**`".to_string())?;
    let close = label
        .find("**")
        .ok_or_else(|| "task label is missing its closing `**`".to_string())?;
    let label = &label[..close];
    let (id, title) = label
        .split_once(" — ")
        .ok_or_else(|| "task label must be `<id> — <title>` using an em dash".to_string())?;
    let id = id.trim();
    let title = title.trim();
    if id.is_empty()
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return Err("task id must use only letters, digits, `.`, `_`, or `-`".into());
    }
    if title.is_empty() {
        return Err("task title must not be empty".into());
    }

    Ok(Some(Header {
        id: id.to_string(),
        title: title.to_string(),
        checked,
        indent: prefix_len,
    }))
}

fn has_unchecked_descendant(tasks: &[Task], index: usize) -> bool {
    tasks
        .iter()
        .skip(index + 1)
        .take_while(|task| task.indent > tasks[index].indent)
        .any(|task| !task.checked)
}

fn has_valid_verify(lines: &[String], task: &Task) -> bool {
    let mut fence: Option<char> = None;
    for (offset, line) in lines[task.line.saturating_sub(1)..task.own_end_line]
        .iter()
        .enumerate()
    {
        let trimmed = line.trim_start();
        if let Some(delimiter) = fence {
            let closes = if delimiter == '`' {
                trimmed.starts_with("```")
            } else {
                trimmed.starts_with("~~~")
            };
            if closes {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") {
            fence = Some('`');
            continue;
        }
        if trimmed.starts_with("~~~") {
            fence = Some('~');
            continue;
        }

        let value = if let Some(value) = trimmed.strip_prefix("Verify:") {
            Some(value)
        } else if offset == 0 {
            line.find("**")
                .and_then(|open| {
                    line[open + 2..]
                        .find("**")
                        .map(|close| open + 2 + close + 2)
                })
                .and_then(|close| line[close..].trim_start().strip_prefix("Verify:"))
        } else {
            None
        };
        if let Some(value) = value {
            let value = value.trim();
            let lower = value.to_ascii_lowercase();
            return !value.is_empty()
                && !value.contains("{{")
                && !value.contains("}}")
                && !matches!(lower.as_str(), "todo" | "tbd" | "replace me");
        }
    }
    false
}

fn bounded_lines(lines: &[String], start: usize, end: usize, max_bytes: usize) -> String {
    let mut out = String::new();
    for line in &lines[start.min(lines.len())..end.min(lines.len())] {
        if out.len() + line.len() + 1 > max_bytes {
            out.push_str("[… excerpt truncated by ralph …]\n");
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_first_unchecked_task_even_after_two_hundred_lines() {
        let mut text = format!("{SCHEMA_MARKER}\n# Backlog\n");
        for _ in 0..220 {
            text.push_str("context\n");
        }
        text.push_str("- [x] **1 — Done.** Verify: yes\n");
        text.push_str("- [ ] **2 — Current.**\n  Verify: cargo test\n");
        text.push_str("- [ ] **3 — Later.**\n  Verify: cargo test\n");
        let doc = Document::parse(&text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        assert_eq!(doc.tasks[doc.selected_index().unwrap()].id, "2");
    }

    #[test]
    fn child_stages_are_selected_in_document_order() {
        let text = format!(
            "{SCHEMA_MARKER}\n- [ ] **36.8 — Parent closure.**\n  Verify: broad suite\n  - [x] **36.8.1 — Schema.**\n    Verify: schema test\n  - [ ] **36.8.2 — Runtime.**\n    Verify: runtime test\n- [ ] **37.1 — Later.**\n  Verify: later test\n"
        );
        let doc = Document::parse(&text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        let selected = doc.selected_index().unwrap();
        assert_eq!(doc.tasks[selected].id, "36.8.2");
        assert_eq!(
            doc.selected_path(selected)
                .iter()
                .map(|t| &t.id)
                .collect::<Vec<_>>(),
            vec!["36.8", "36.8.2"]
        );
        assert!(!doc.own_excerpt(selected, 4_000).contains("37.1"));
    }

    #[test]
    fn parent_becomes_closure_step_after_children_finish() {
        let text = format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — Close parent.**\n  Verify: broad suite\n  - [x] **1.1 — Done.** Verify: focused\n"
        );
        let doc = Document::parse(&text);
        assert_eq!(doc.tasks[doc.selected_index().unwrap()].id, "1");
    }

    #[test]
    fn structural_and_verification_errors_are_reported() {
        let text = format!(
            "{SCHEMA_MARKER}\n- [x] **1 — Parent.**\n   - [ ] **other — Child.**\n- [ ] **1 — Duplicate.**\n"
        );
        let doc = Document::parse(&text);
        let rendered = doc
            .issues
            .iter()
            .map(|issue| format!("{}: {}", issue.line, issue.message))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("indentation"));
        assert!(rendered.contains("parent prefix"));
        assert!(rendered.contains("checked parent"));
        assert!(rendered.contains("duplicate task id"));
        assert!(rendered.contains("needs a non-placeholder `Verify:`"));
    }

    #[test]
    fn malformed_checkbox_is_not_silently_ignored() {
        let doc = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] not bold\n- [maybe] **2 — Nope.**\n"
        ));
        assert!(doc.has_errors());
        assert!(doc
            .issues
            .iter()
            .any(|issue| issue.message.contains("bold")));
        assert!(doc
            .issues
            .iter()
            .any(|issue| issue.message.contains("checkbox")));
    }

    #[test]
    fn absent_marker_is_a_warning_not_an_error() {
        let doc = Document::parse("- [ ] **1 — Work.** Verify: test\n");
        assert!(!doc.has_errors());
        assert!(!doc.schema_present);
        assert!(doc
            .issues
            .iter()
            .any(|issue| issue.severity == Severity::Warning));
    }

    #[test]
    fn compatibility_mode_aggregates_missing_verify_as_a_warning() {
        let doc = Document::parse("- [ ] **1 — Work.**\n- [ ] **2 — More work.**\n");
        assert!(!doc.has_errors());
        assert!(doc
            .issues
            .iter()
            .any(|issue| issue.message.contains("2 pending task(s)")));
    }

    #[test]
    fn checkbox_examples_inside_code_fences_are_ignored() {
        let text = format!(
            "{SCHEMA_MARKER}\n```markdown\n- [ ] not a real task\n```\n- [ ] **1 — Real.** Verify: test\n"
        );
        let doc = Document::parse(&text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        assert_eq!(doc.tasks.len(), 1);
        assert_eq!(doc.tasks[0].id, "1");
    }

    #[test]
    fn markdown_links_are_not_malformed_tasks() {
        let text = format!(
            "{SCHEMA_MARKER}\n- [documentation](https://example.test)\n- [x](https://example.test/x)\n- [ ] **1 — Real.** Verify: test\n"
        );
        let doc = Document::parse(&text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        assert_eq!(doc.tasks.len(), 1);
    }

    #[test]
    fn alternate_checkbox_bullets_and_unclosed_fences_are_errors() {
        let alternate = Document::parse(&format!(
            "{SCHEMA_MARKER}\n* [ ] **1 — Wrong bullet.** Verify: test\n"
        ));
        assert!(alternate.has_errors());
        assert!(alternate
            .issues
            .iter()
            .any(|issue| issue.message.contains("must use the `-` bullet")));

        let fence = Document::parse(&format!(
            "{SCHEMA_MARKER}\n```markdown\n- [ ] **1 — Hidden.** Verify: test\n"
        ));
        assert!(fence.has_errors());
        assert!(fence
            .issues
            .iter()
            .any(|issue| issue.message.contains("unclosed fenced")));
    }

    #[test]
    fn marked_empty_backlog_is_not_complete() {
        let doc = Document::parse(&format!("{SCHEMA_MARKER}\n# Backlog\nNo tasks here.\n"));
        assert!(doc.has_errors());
        assert_eq!(doc.selected_index(), None);
        assert!(doc
            .issues
            .iter()
            .any(|issue| issue.message.contains("no schema tasks")));
    }

    #[test]
    fn verify_must_be_a_real_field_outside_examples() {
        let placeholder = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — Work.**\n  Verify: {{{{fill me}}}}\n"
        ));
        assert!(placeholder.has_errors());

        let prose = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — Work.**\n  This is missing Verify: on purpose.\n"
        ));
        assert!(prose.has_errors());

        let fenced = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — Work.**\n  ```text\n  Verify: fake\n  ```\n"
        ));
        assert!(fenced.has_errors());
    }

    #[test]
    fn task_excerpt_stops_at_following_heading() {
        let text = format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — Work.**\n  Verify: test\n## Later phase\ncontext that is not task 1\n"
        );
        let doc = Document::parse(&text);
        let excerpt = doc.own_excerpt(0, 4_000);
        assert!(!excerpt.contains("Later phase"));
        assert!(!excerpt.contains("not task 1"));
    }

    #[test]
    fn unknown_schema_version_is_an_error() {
        let doc = Document::parse("<!-- ralph-backlog: v9 -->\n- [ ] **1 — Work.** Verify: test\n");
        assert!(doc.has_errors());
        assert!(doc
            .issues
            .iter()
            .any(|issue| issue.message.contains("unsupported")));
    }

    #[test]
    fn completed_work_after_pending_sibling_is_warned() {
        let text = format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — Current.** Verify: test\n- [x] **2 — Skipped ahead.** Verify: test\n- [x] **3 — Also ahead.** Verify: test\n"
        );
        let doc = Document::parse(&text);
        assert!(!doc.has_errors());
        let warnings = doc
            .issues
            .iter()
            .filter(|issue| issue.severity == Severity::Warning)
            .collect::<Vec<_>>();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("document order was bypassed"));
    }
}
