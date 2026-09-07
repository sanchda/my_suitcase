//! The deliberately small Markdown schema used by Ralph backlogs.
//!
//! Executable work is a checkbox whose bold label is `<id> — <title>`,
//! optionally followed by a `@tier — ` decoration slot. Two-space-indented
//! child checkboxes are ordered stages. The first unchecked task with no
//! unchecked descendants is the next executable leaf.

use std::collections::{HashMap, HashSet};

pub const SCHEMA_MARKER: &str = "<!-- ralph-backlog: v2 -->";

/// Model tiers recognized in a task's `@tier` decoration slot.
pub(crate) const MODEL_TIERS: [&str; 3] = ["haiku", "sonnet", "opus"];

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
    pub tier: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Document {
    lines: Vec<String>,
    pub tasks: Vec<Task>,
    pub issues: Vec<Issue>,
    pub schema_present: bool,
}

/// The line span of the fully-completed leading run of top-level sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixSpan {
    pub section_count: usize,
    pub first_line: usize, // 1-based line of the first top-level task
    pub last_line: usize,  // 1-based subtree-end line of the last swept section
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

            // A bad decoration is reported without dropping the task: its id and
            // title parsed, and deleting it would silently move the routing
            // target while the author is still fixing the typo.
            if let Some(message) = &header.decoration_error {
                issues.push(Issue::error(line_no, message.clone()));
            }
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
                tier: header.tier,
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
                        "missing schema marker `{SCHEMA_MARKER}`; parsed in compatibility mode"
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

        // Pending only, as with the `Verify:` contract check: a completed task's
        // tier can never route anything again, so demanding it be migrated is
        // busywork on a historical record rather than a guard against misrouting.
        for task in tasks.iter().filter(|task| !task.checked) {
            for (line_no, tier) in leftover_v1_decorations(&lines, task) {
                issues.push(Issue::error(
                    line_no,
                    format!(
                        "`({tier}…)` is v1 tier syntax that v2 does not honor; move the tier to the `@{tier} — ` slot right after the task's bold label"
                    ),
                ));
            }
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
                    "{} pending task(s) lack a non-placeholder `Verify:` contract; compatibility mode permits this, but a marked backlog will reject it",
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

    /// Count of pending executable leaves — a point-in-time proxy for remaining
    /// work, used for the `iter N/M` progress estimate. Same predicate as
    /// [`Self::selected_index`]: an unchecked task with no unchecked descendant.
    pub fn pending_leaf_count(&self) -> usize {
        self.tasks
            .iter()
            .enumerate()
            .filter(|(index, task)| !task.checked && !has_unchecked_descendant(&self.tasks, *index))
            .count()
    }

    /// The maximal leading run of top-level sections that are fully complete
    /// (own box `[x]` and no unchecked descendant). `None` when the first
    /// top-level section is not fully complete, i.e. nothing safe to sweep.
    pub fn completed_leading_prefix(&self) -> Option<PrefixSpan> {
        let tops: Vec<usize> = self
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.parent.is_none())
            .map(|(i, _)| i)
            .collect();
        let first_line = self.tasks.get(*tops.first()?)?.line;
        let mut count = 0;
        let mut last_line = 0;
        for &idx in &tops {
            let sweepable = self.tasks[idx].checked && !has_unchecked_descendant(&self.tasks, idx);
            if !sweepable {
                break;
            }
            count += 1;
            last_line = self.tasks[idx].end_line;
        }
        if count == 0 {
            None
        } else {
            Some(PrefixSpan {
                section_count: count,
                first_line,
                last_line,
            })
        }
    }

    /// The model tier declared in the task's own `@tier` slot
    /// (e.g. `**1 — Rework.** @opus — …` → `opus`), or `None`. Advisory
    /// routing, not a spec field.
    pub fn model_hint(&self, index: usize) -> Option<String> {
        self.tasks.get(index)?.tier.clone()
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

    /// Up to `n` upcoming executable-leaf labels ("id — title"), in document
    /// order from the current frontier. Feeds the handoff synthesizer.
    pub fn upcoming_leaf_labels(&self, n: usize) -> Vec<String> {
        self.tasks
            .iter()
            .enumerate()
            .filter(|(i, t)| !t.checked && !has_unchecked_descendant(&self.tasks, *i))
            .take(n)
            .map(|(_, t)| format!("{} — {}", t.id, t.title))
            .collect()
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
    tier: Option<String>,
    decoration_error: Option<String>,
}

/// Names the character found where the ` — ` delimiter belongs. An en dash, an
/// ASCII hyphen and a space-less em dash are the three likely typos, and in a
/// terminal they are indistinguishable from the real thing — echoing the input
/// would show the author the character they believe they already typed.
fn delimiter_error(cursor: &str) -> String {
    const EXPECTED: &str =
        "a `@tier` decoration must be followed by ` — ` (em dash, U+2014) before the task prose";
    match cursor.chars().next() {
        None => format!("{EXPECTED}; found end of line"),
        Some('—') => {
            format!("{EXPECTED}; the em dash is there but the space after it is missing")
        }
        Some('–') => format!("{EXPECTED}; found an en dash `–` (U+2013)"),
        Some('-') => format!("{EXPECTED}; found an ASCII hyphen `-` (U+002D)"),
        Some(found) => format!("{EXPECTED}; found `{found}`"),
    }
}

/// Splits the header text following the label's closing `**` into its optional
/// `@tier` decoration and the prose after it. The slot is position-anchored and
/// em-dash terminated, so a typo inside the slot is a lint error rather than a
/// silently dropped route, and parentheses in the body stay inert. It only fires
/// on the label's *first* closing `**`: nested bold in a title ends the label
/// early, and a decoration after that is never reached.
fn split_decoration(rest: &str) -> Result<(Option<String>, &str), String> {
    let rest = rest.trim_start();
    if !rest.starts_with(['@', '!']) {
        return Ok((None, rest));
    }
    let mut tiers: Vec<&str> = Vec::new();
    let mut cursor = rest;
    while cursor.starts_with(['@', '!']) {
        let after_at = cursor.strip_prefix('@').unwrap_or(cursor);
        let end = after_at
            .find(|ch: char| ch.is_whitespace() || ch == '—' || ch == '–')
            .unwrap_or(after_at.len());
        let word = &after_at[..end];
        let model = word.strip_prefix('!').unwrap_or(word);
        if !crate::backend::valid_model(word) || crate::backend::infer_model(model).is_none() {
            return Err(format!(
                "unknown decoration `@{word}`; expected a model such as @opus, @astra, !astra, or !fable"
            ));
        }
        tiers.push(word);
        cursor = after_at[end..].trim_start();
    }
    if tiers.len() > 1 {
        return Err(format!(
            "conflicting tier decorations `@{}`; a task may declare at most one model tier",
            tiers.join("` and `@")
        ));
    }
    let prose = cursor
        .strip_prefix('—')
        .filter(|after| after.is_empty() || after.starts_with(char::is_whitespace))
        .ok_or_else(|| delimiter_error(cursor))?;
    Ok((Some(tiers[0].to_string()), prose.trim_start()))
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

    let body = rest
        .strip_prefix("**")
        .ok_or_else(|| "task label must be bold: `**<id> — <title>**`".to_string())?;
    let close = body
        .find("**")
        .ok_or_else(|| "task label is missing its closing `**`".to_string())?;
    // A `**` inside the title would otherwise steal the label's closing
    // delimiter, silently truncating the title and dropping the tier decoration
    // with it. The boundary is only unambiguous when nothing runs on from it, so
    // refuse the line rather than guess which `**` was meant.
    if let Some(ch) = body[close + 2..].chars().next() {
        if !ch.is_whitespace() {
            return Err(format!(
                "task label's closing `**` must be followed by a space or end of line; found `{ch}` — a title cannot contain `**`"
            ));
        }
    }
    let label = &body[..close];
    let (tier, decoration_error) = match split_decoration(&body[close + 2..]) {
        Ok((tier, _)) => (tier, None),
        Err(message) => (None, Some(message)),
    };
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
        tier,
        decoration_error,
    }))
}

fn has_unchecked_descendant(tasks: &[Task], index: usize) -> bool {
    tasks
        .iter()
        .skip(index + 1)
        .take_while(|task| task.indent > tasks[index].indent)
        .any(|task| !task.checked)
}

/// The task's own lines — header plus prose, excluding child stages — paired
/// with their offset from the header and with fenced blocks dropped, so an
/// example inside a fence never speaks for the task that contains it.
fn unfenced_own_lines<'a>(lines: &'a [String], task: &Task) -> Vec<(usize, &'a str)> {
    let mut out = Vec::new();
    let mut fence: Option<char> = None;
    for (offset, line) in lines[task.line.saturating_sub(1)..task.own_end_line.min(lines.len())]
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
        out.push((offset, line.as_str()));
    }
    out
}

fn paren_groups(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find('(') {
        rest = &rest[open + 1..];
        match rest.find(')') {
            Some(close) => {
                out.push(&rest[..close]);
                rest = &rest[close + 1..];
            }
            None => break,
        }
    }
    out
}

/// v1 read the model tier from a parenthetical anywhere in the task body. Under
/// v2 a leftover one is inert prose — a route dropped in silence — so it is an
/// error until migrated. Self-limiting: it cannot fire on a migrated backlog.
/// The leading token rule is v1's own, so exactly what v1 honored trips it.
fn leftover_v1_decorations(lines: &[String], task: &Task) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (offset, line) in unfenced_own_lines(lines, task) {
        for group in paren_groups(line) {
            let leading = group
                .split(|ch: char| !ch.is_ascii_alphanumeric())
                .find(|token| !token.is_empty());
            if let Some(tier) = leading.filter(|token| MODEL_TIERS.contains(token)) {
                out.push((task.line + offset, tier.to_string()));
            }
        }
    }
    out
}

fn has_valid_verify(lines: &[String], task: &Task) -> bool {
    for (offset, line) in unfenced_own_lines(lines, task) {
        let trimmed = line.trim_start();
        let value = if let Some(value) = trimmed.strip_prefix("Verify:") {
            Some(value)
        } else if offset == 0 {
            line.find("**")
                .and_then(|open| {
                    line[open + 2..]
                        .find("**")
                        .map(|close| open + 2 + close + 2)
                })
                .and_then(|close| split_decoration(&line[close..]).ok())
                .and_then(|(_, prose)| prose.strip_prefix("Verify:"))
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
    fn model_hint_reads_tier_decoration() {
        let text = format!(
            "{SCHEMA_MARKER}\n# H\n- [ ] **1 — A.** @opus — do a thing.\n  Verify: y\n- [ ] **2 — B.** @haiku — cleanup.\n  Verify: y\n- [ ] **3 — C.** review.\n  Verify: y\n"
        );
        let doc = Document::parse(&text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        let idx = |id: &str| doc.tasks.iter().position(|t| t.id == id).unwrap();
        assert_eq!(doc.model_hint(idx("1")).as_deref(), Some("opus"));
        assert_eq!(doc.model_hint(idx("2")).as_deref(), Some("haiku"));
        assert_eq!(doc.model_hint(idx("3")), None);
    }

    #[test]
    fn model_annotations_support_exclusive_and_concrete_models() {
        for (annotation, expected) in [
            ("!astra", "!astra"),
            ("!fable", "!fable"),
            ("@!fable", "!fable"),
            ("@astra", "astra"),
            ("@claude-fable-5-1", "claude-fable-5-1"),
        ] {
            let doc = Document::parse(&format!(
                "{SCHEMA_MARKER}\n- [ ] **1 — Work.** {annotation} — implement.\n  Verify: true\n"
            ));
            assert_eq!(doc.model_hint(0).as_deref(), Some(expected));
        }
        for annotation in ["!", "!!astra", "!fable !astra", "!fable @opus"] {
            assert!(split_decoration(&format!("{annotation} — implement.")).is_err());
        }
    }

    #[test]
    fn parent_and_child_carry_independent_tiers() {
        let text = format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — Parent.** @sonnet — covers both halves.\n  Verify: broad suite\n  - [ ] **1.1 — Child.** @opus — stuff.\n    Verify: focused\n"
        );
        let doc = Document::parse(&text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        let idx = |id: &str| doc.tasks.iter().position(|t| t.id == id).unwrap();
        assert_eq!(doc.model_hint(idx("1")).as_deref(), Some("sonnet"));
        assert_eq!(doc.model_hint(idx("1.1")).as_deref(), Some("opus"));
    }

    #[test]
    fn prose_parentheses_are_inert() {
        let text = format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — X.** rework (code.) bits.\n  Leave the (unchanged) half alone.\n  Verify: y\n"
        );
        let doc = Document::parse(&text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        assert_eq!(doc.model_hint(0), None);
    }

    #[test]
    fn leftover_v1_parenthetical_in_a_body_line_is_an_error() {
        // The exact shape that regressed: a v1 decoration at the end of a body
        // line, which v2 would otherwise read as inert prose.
        let doc = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — X.** do a thing.\n  Rework the base. (opus/pedagogy.)\n  Verify: y\n"
        ));
        assert!(doc.has_errors(), "{:?}", doc.issues);
        let issue = doc
            .issues
            .iter()
            .find(|issue| issue.message.contains("v1 tier syntax"))
            .expect("v1 leftover reported");
        assert_eq!(issue.severity, Severity::Error);
        assert_eq!(issue.line, 3);
        assert!(issue.message.contains("@opus"), "{}", issue.message);
    }

    #[test]
    fn an_ambiguous_label_close_is_an_error_not_a_truncated_title() {
        // `**` inside a title used to steal the label's closing delimiter, which
        // silently truncated the title AND dropped the tier with a clean lint.
        for line in [
            "- [ ] **1 — Make **all** paths safe.** @opus — big change.",
            "- [ ] **2 — Fix `**kwargs` handling.** @opus — big change.",
        ] {
            let doc = Document::parse(&format!("{SCHEMA_MARKER}\n{line}\n  Verify: y\n"));
            assert!(doc.has_errors(), "{line} parsed silently: {:?}", doc.issues);
            assert!(
                doc.issues
                    .iter()
                    .any(|issue| issue.message.contains("closing `**`")),
                "{line}: {:?}",
                doc.issues
            );
        }
    }

    #[test]
    fn bold_in_the_prose_after_the_label_is_still_fine() {
        // The legitimate case the stricter rule must not break: emphasis in the
        // prose, well after the label has closed.
        let doc = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **3 — Plain title.** @opus — this is **very** important.\n  Verify: y\n"
        ));
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        assert_eq!(doc.tasks[0].title, "Plain title.");
        assert_eq!(doc.model_hint(0).as_deref(), Some("opus"));
    }

    #[test]
    fn a_completed_task_keeps_its_v1_parenthetical() {
        // Migration must not demand edits to historical records: a checked task
        // will never be routed again, so its stale tier costs nothing.
        let doc = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [x] **1 — Done.** shipped it. (opus/pedagogy.) Verify: y\n- [ ] **2 — Next.** @sonnet — go.\n  Verify: y\n"
        ));
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        assert_eq!(doc.model_hint(1).as_deref(), Some("sonnet"));
    }

    #[test]
    fn leftover_v1_parenthetical_on_the_header_line_is_an_error() {
        let doc = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — X.** big. (opus — shared-base refactor.)\n  Verify: y\n"
        ));
        assert!(doc
            .issues
            .iter()
            .any(|issue| issue.message.contains("v1 tier syntax")));
    }

    #[test]
    fn a_tier_parenthetical_inside_a_fence_is_not_a_leftover() {
        let doc = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — X.** do a thing.\n  ```bash\n  echo (opus) # a shell snippet, not a decoration\n  ```\n  Verify: y\n"
        ));
        assert!(!doc.has_errors(), "{:?}", doc.issues);
    }

    #[test]
    fn a_child_stage_leftover_does_not_charge_its_parent() {
        let doc = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — Parent.** covers (unchanged) halves.\n  Verify: broad\n  - [ ] **1.1 — Child.** stuff. (opus.)\n    Verify: focused\n"
        ));
        let leftovers: Vec<usize> = doc
            .issues
            .iter()
            .filter(|issue| issue.message.contains("v1 tier syntax"))
            .map(|issue| issue.line)
            .collect();
        assert_eq!(leftovers, vec![4]);
    }

    #[test]
    fn an_at_sign_in_the_body_is_prose_not_a_decoration() {
        let text = format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — X.** do the thing.\n  @opus would be nice, but this line is prose.\n  Verify: y\n"
        );
        let doc = Document::parse(&text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        assert_eq!(doc.model_hint(0), None);
    }

    #[test]
    fn misspelled_tier_is_an_error_not_prose() {
        let doc = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — X.** @opuss — x\n  Verify: y\n"
        ));
        assert!(doc.has_errors());
        assert!(doc
            .issues
            .iter()
            .any(|issue| issue.message.contains("unknown decoration")));
    }

    #[test]
    fn tier_decoration_must_be_closed_by_an_em_dash() {
        let doc = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — X.** @opus big change.\n  Verify: y\n"
        ));
        assert!(doc.has_errors());
        assert!(doc
            .issues
            .iter()
            .any(|issue| issue.message.contains("must be followed by ` — `")));
    }

    #[test]
    fn near_miss_delimiters_name_the_character_that_was_found() {
        let message = |line: &str| {
            let doc = Document::parse(&format!("{SCHEMA_MARKER}\n{line}\n  Verify: y\n"));
            doc.issues
                .iter()
                .find(|issue| issue.message.contains("`@tier`"))
                .unwrap_or_else(|| panic!("no delimiter error for {line}"))
                .message
                .clone()
        };

        let en_dash = message("- [ ] **1 — X.** @opus – big change.");
        assert!(en_dash.contains("en dash"), "{en_dash}");
        assert!(en_dash.contains("U+2013"), "{en_dash}");

        let hyphen = message("- [ ] **1 — X.** @opus - big change.");
        assert!(hyphen.contains("hyphen"), "{hyphen}");

        let unspaced = message("- [ ] **1 — X.** @opus—big change.");
        assert!(unspaced.contains("space"), "{unspaced}");
        assert!(!unspaced.contains("en dash"), "{unspaced}");

        // Every variant names the character the author was supposed to type.
        for message in [en_dash, hyphen, unspaced] {
            assert!(message.contains("U+2014"), "{message}");
        }
    }

    #[test]
    fn two_tier_decorations_are_an_error() {
        let doc = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — X.** @opus @sonnet — x\n  Verify: y\n"
        ));
        assert!(doc.has_errors());
        assert!(doc
            .issues
            .iter()
            .any(|issue| issue.message.contains("conflicting tier decorations")));
    }

    #[test]
    fn a_decoration_error_keeps_the_task_routable() {
        // A typo in the slot must not delete the task: routing would then point
        // at the next one and brief a target the author never chose.
        let doc = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — First.** @opuss — x\n  Verify: y\n- [ ] **2 — Second.**\n  Verify: y\n"
        ));
        assert!(doc.has_errors());
        assert_eq!(
            doc.tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            vec!["1", "2"]
        );
        assert_eq!(doc.tasks[0].title, "First.");
        assert_eq!(doc.tasks[0].tier, None);
        assert_eq!(doc.tasks[doc.selected_index().unwrap()].id, "1");
        assert!(doc
            .issues
            .iter()
            .any(|issue| issue.line == 2 && issue.message.contains("unknown decoration")));
    }

    #[test]
    fn v1_marker_is_unsupported() {
        let doc = Document::parse("<!-- ralph-backlog: v1 -->\n- [ ] **1 — Work.** Verify: test\n");
        assert!(doc.has_errors());
        assert!(doc
            .issues
            .iter()
            .any(|issue| issue.message.contains("unsupported") && issue.message.contains("v2")));
    }

    #[test]
    fn tier_decoration_may_precede_an_inline_verify() {
        let doc = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — X.** @opus — Verify: cargo test\n"
        ));
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        assert_eq!(doc.model_hint(0).as_deref(), Some("opus"));
        assert!(has_valid_verify(&doc.lines, &doc.tasks[0]));
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
    fn completed_leading_prefix_stops_at_first_pending_section() {
        let mut text = String::from(SCHEMA_MARKER);
        text.push('\n');
        text.push_str("# Backlog\n\n");
        text.push_str("- [x] **1 — Done.** Verify: yes\n");
        text.push_str("  - [x] **1.1 — Sub.** Verify: yes\n");
        text.push_str("- [x] **2 — Also done.** Verify: yes\n");
        text.push_str("- [ ] **3 — Pending.** Verify: yes\n");
        text.push_str("- [x] **4 — Later done.** Verify: yes\n");
        let doc = Document::parse(&text);
        let prefix = doc.completed_leading_prefix().expect("has a prefix");
        assert_eq!(prefix.section_count, 2); // 1 and 2, not 4 (after pending 3)
        let two = doc.tasks.iter().find(|t| t.id == "2").unwrap();
        assert_eq!(prefix.last_line, two.end_line);
        assert_eq!(prefix.first_line, doc.tasks[0].line);
    }

    #[test]
    fn no_prefix_when_first_section_pending() {
        let mut text = String::from(SCHEMA_MARKER);
        text.push_str("\n# Backlog\n\n- [ ] **1 — Pending.** Verify: yes\n");
        let doc = Document::parse(&text);
        assert!(doc.completed_leading_prefix().is_none());
    }

    #[test]
    fn no_prefix_when_parent_closure_leaf_unchecked() {
        // Children done but the parent's own box is not → not sweepable.
        let mut text = String::from(SCHEMA_MARKER);
        text.push_str("\n# Backlog\n\n");
        text.push_str("- [ ] **1 — Parent closure pending.** Verify: yes\n");
        text.push_str("  - [x] **1.1 — Sub.** Verify: yes\n");
        let doc = Document::parse(&text);
        assert!(doc.completed_leading_prefix().is_none());
    }

    #[test]
    fn upcoming_leaf_labels_are_leaves_in_order() {
        let doc = Document::parse(&format!(
            "{SCHEMA_MARKER}\n# B\n\n- [ ] **1 — One.** Verify: y\n- [ ] **2 — Two.** Verify: y\n- [ ] **3 — Three.** Verify: y\n"
        ));
        assert_eq!(
            doc.upcoming_leaf_labels(2),
            vec!["1 — One.".to_string(), "2 — Two.".to_string()]
        );
    }

    #[test]
    fn pending_leaf_count_counts_executable_leaves() {
        // Three top-level pending leaves.
        let flat = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — One.** Verify: y\n- [ ] **2 — Two.** Verify: y\n- [ ] **3 — Three.** Verify: y\n"
        ));
        assert_eq!(flat.pending_leaf_count(), 3);

        // A parent with pending children counts its children (leaves), not itself.
        let nested = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — Parent.** Verify: broad\n  - [ ] **1.1 — A.** Verify: y\n  - [ ] **1.2 — B.** Verify: y\n"
        ));
        assert_eq!(nested.pending_leaf_count(), 2);

        // All children done → parent becomes its own closure leaf (count 1).
        let closure = Document::parse(&format!(
            "{SCHEMA_MARKER}\n- [ ] **1 — Parent.** Verify: broad\n  - [x] **1.1 — A.** Verify: y\n"
        ));
        assert_eq!(closure.pending_leaf_count(), 1);

        // Fully complete → nothing pending.
        let done = Document::parse(&format!("{SCHEMA_MARKER}\n- [x] **1 — Done.** Verify: y\n"));
        assert_eq!(done.pending_leaf_count(), 0);
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
