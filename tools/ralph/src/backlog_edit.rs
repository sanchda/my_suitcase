//! Schema-safe backlog mutation. Every operation is a pure
//! `String -> Result<String, String>` transform gated by an in-memory lint: an
//! edit that would make the backlog invalid is REJECTED and never reaches disk,
//! so a mutation can never crash a running loop (which aborts on an invalid
//! backlog). Writes are atomic (temp file + rename) and go through the inbox —
//! see `inbox.rs` for who applies them.

use crate::backlog::{Document, Severity, Task, SCHEMA_MARKER};
use crate::inbox::Request;
use crate::R;

/// The skeleton a bootstrapped backlog starts from. `add` appends the first
/// task to this when the backlog file is absent — completion archives the file
/// away, and the next arc must be startable from `ralph add` alone.
pub fn empty_backlog() -> String {
    format!("{SCHEMA_MARKER}\n# Backlog\n")
}

/// Collected lint error lines for a rejected mutation.
fn lint_errors(doc: &Document) -> String {
    doc.issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| format!("  {}: {}", i.line, i.message))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Next integer top-level id: max numeric top-level id + 1, else "1".
fn next_top_level_id(doc: &Document) -> String {
    let max = doc
        .tasks
        .iter()
        .filter(|t| t.indent == 0)
        .filter_map(|t| t.id.parse::<u64>().ok())
        .max();
    (max.unwrap_or(0) + 1).to_string()
}

/// Next free `<parent>.N`, counting only direct children.
fn next_child_id(doc: &Document, parent_id: &str) -> String {
    let prefix = format!("{parent_id}.");
    let max = doc
        .tasks
        .iter()
        .filter_map(|t| t.id.strip_prefix(&prefix))
        .filter(|rest| !rest.contains('.'))
        .filter_map(|rest| rest.parse::<u64>().ok())
        .max();
    format!("{parent_id}.{}", max.unwrap_or(0) + 1)
}

/// The one-line body form: `--verify` is shorthand for a body of just a contract.
pub fn verify_body(verify: &str) -> String {
    format!("Verify: {}", verify.trim())
}

/// Header plus body at `indent`. Body lines keep their relative shape but are
/// re-anchored two spaces under the header, per the schema.
fn task_block(indent: usize, id: &str, title: &str, body: &str) -> String {
    let pad = " ".repeat(indent);
    let body = body.trim_matches('\n');
    let base = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out = format!("{pad}- [ ] **{id} — {}**\n", title.trim());
    for line in body.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            out.push('\n');
        } else {
            out.push_str(&format!("{pad}  {}\n", &line[base.min(line.len())..]));
        }
    }
    out
}

/// Splice `block` in after `parent`'s subtree, skipping back over trailing blank
/// lines so the new stage lands snug against its siblings.
fn insert_after_subtree(current: &str, parent: &Task, block: &str) -> String {
    let lines: Vec<&str> = current.lines().collect();
    let mut at = parent.end_line.min(lines.len());
    while at > parent.line && lines[at - 1].trim().is_empty() {
        at -= 1;
    }
    let mut out = String::new();
    for line in &lines[..at] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(block);
    for line in &lines[at..] {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The lint gate every add passes through: a result with errors never returns.
fn gated(text: String, id: &str) -> Result<(String, String), String> {
    let doc = Document::parse(&text);
    if doc.has_errors() {
        return Err(lint_errors(&doc));
    }
    Ok((text, id.to_string()))
}

fn check_title(title: &str) -> Result<(), String> {
    if title.contains("**") {
        return Err("task title may not contain `**` (it breaks the bold label)".to_string());
    }
    Ok(())
}

/// Append a task at the end of the backlog under the next top-level id; returns
/// `(new_text, new_id)` or the lint errors that would result.
pub fn apply_add_top(current: &str, title: &str, body: &str) -> Result<(String, String), String> {
    check_title(title)?;
    let id = next_top_level_id(&Document::parse(current));
    let mut text = current.trim_end().to_string();
    text.push('\n');
    text.push_str(&task_block(0, &id, title, body));
    gated(text, &id)
}

/// Insert an explicitly numbered task as the last child of the parent its id
/// implies (`3.1.1` → under `3.1`); a dotless id appends at top level.
pub fn apply_add_with_id(
    current: &str,
    id: &str,
    title: &str,
    body: &str,
) -> Result<(String, String), String> {
    check_title(title)?;
    let doc = Document::parse(current);
    if let Some(existing) = doc.tasks.iter().find(|t| t.id == id) {
        return Err(format!("id {id} already exists (line {})", existing.line));
    }
    let Some((parent_id, _)) = id.rsplit_once('.') else {
        let mut text = current.trim_end().to_string();
        text.push('\n');
        text.push_str(&task_block(0, id, title, body));
        return gated(text, id);
    };
    let parent = doc
        .tasks
        .iter()
        .find(|t| t.id == parent_id)
        .ok_or_else(|| format!("no task with id `{parent_id}` (the parent implied by `{id}`)"))?;
    let block = task_block(parent.indent + 2, id, title, body);
    gated(insert_after_subtree(current, parent, &block), id)
}

/// Add a stage under `parent_id`, numbering it so the caller doesn't have to.
pub fn apply_add_under(
    current: &str,
    parent_id: &str,
    title: &str,
    body: &str,
) -> Result<(String, String), String> {
    let doc = Document::parse(current);
    if !doc.tasks.iter().any(|t| t.id == parent_id) {
        return Err(format!("no task with id `{parent_id}`"));
    }
    apply_add_with_id(current, &next_child_id(&doc, parent_id), title, body)
}

/// Check off one task. Deliberately no cascade: a parent whose last child closes
/// is a container that still owes its own integration step.
pub fn apply_done(current: &str, id: &str) -> Result<String, String> {
    let doc = Document::parse(current);
    let task = doc
        .tasks
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("no task with id `{id}`"))?;
    if task.checked {
        return Err(format!("task `{id}` is already checked"));
    }
    let mut lines: Vec<String> = current.lines().map(String::from).collect();
    let line = &mut lines[task.line - 1];
    *line = line.replacen("- [ ] ", "- [x] ", 1);
    let mut out = lines.join("\n");
    out.push('\n');
    let new_doc = Document::parse(&out);
    if new_doc.has_errors() {
        return Err(lint_errors(&new_doc));
    }
    Ok(out)
}

/// Remove a task's own body and its subtree; returns `(new_text, removed_block)`
/// so the caller can archive what it deleted. Children need `recursive` — a
/// silent cascade is how work disappears.
pub fn apply_drop(current: &str, id: &str, recursive: bool) -> Result<(String, String), String> {
    let doc = Document::parse(current);
    let index = doc
        .tasks
        .iter()
        .position(|t| t.id == id)
        .ok_or_else(|| format!("no task with id `{id}`"))?;
    let children = doc.tasks.iter().filter(|t| t.parent == Some(index)).count();
    if children > 0 && !recursive {
        return Err(format!(
            "task `{id}` has {children} child stage(s) — pass --recursive to drop them too"
        ));
    }
    let task = &doc.tasks[index];
    let lines: Vec<&str> = current.lines().collect();
    let start = task.line.saturating_sub(1);
    let end = task.end_line.min(lines.len());
    let removed = format!("{}\n", lines[start..end].join("\n"));
    let mut out = String::new();
    for line in lines[..start].iter().chain(&lines[end..]) {
        out.push_str(line);
        out.push('\n');
    }
    let new_doc = Document::parse(&out);
    if new_doc.has_errors() {
        return Err(lint_errors(&new_doc));
    }
    Ok((out, removed))
}

/// Un-check task `id` (flip its `[x]` back to `[ ]`), plus any checked ancestor
/// — a leaf that closed its parent must reopen it or the result fails the
/// "checked parent contains an unchecked stage" lint. Same lint-or-reject
/// safety as add/edit: an invalid result never reaches disk.
pub fn apply_uncheck(current: &str, id: &str) -> Result<String, String> {
    let doc = Document::parse(current);
    let index = doc
        .tasks
        .iter()
        .position(|t| t.id == id)
        .ok_or_else(|| format!("no task with id `{id}`"))?;
    if !doc.tasks[index].checked {
        return Err(format!("task `{id}` is not checked"));
    }
    // Header lines to flip: the task itself and every checked ancestor.
    let mut header_lines = vec![doc.tasks[index].line];
    let mut parent = doc.tasks[index].parent;
    while let Some(p) = parent {
        if doc.tasks[p].checked {
            header_lines.push(doc.tasks[p].line);
        }
        parent = doc.tasks[p].parent;
    }
    let mut lines: Vec<String> = current.lines().map(String::from).collect();
    for line_no in header_lines {
        let line = &mut lines[line_no - 1];
        let flipped = line.replacen("- [x] ", "- [ ] ", 1);
        *line = if flipped != *line {
            flipped
        } else {
            line.replacen("- [X] ", "- [ ] ", 1)
        };
    }
    let mut out = lines.join("\n");
    out.push('\n');
    let new_doc = Document::parse(&out);
    if new_doc.has_errors() {
        return Err(lint_errors(&new_doc));
    }
    Ok(out)
}

/// Atomically replace `path`'s contents with `new_text` (temp file + rename in
/// the same directory, so a reader never sees a half-written backlog).
pub(crate) fn write_atomic(path: &std::path::Path, new_text: &str) -> R<()> {
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("BACKLOG.md"),
        std::process::id()
    ));
    std::fs::write(&tmp, new_text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// A simple `--flag value` extractor for the subcommand's own grammar.
fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

/// Replace a task's OWN body (header + own prose, excluding child stages) with a
/// regenerated `- [ ] **id — title**` / `Verify:` pair at the same indent and
/// checked state. v1 scope: text replacement only — no re-parenting/reordering.
pub fn apply_edit(current: &str, id: &str, title: &str, verify: &str) -> Result<String, String> {
    let doc = Document::parse(current);
    let task = doc
        .tasks
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("no task with id `{id}`"))?;
    if title.contains("**") {
        return Err("task title may not contain `**` (it breaks the bold label)".to_string());
    }
    let indent = " ".repeat(task.indent);
    let checkbox = if task.checked { "x" } else { " " };
    let new_body = format!(
        "{indent}- [{checkbox}] **{id} — {}**\n{indent}  Verify: {}\n",
        title.trim(),
        verify.trim()
    );
    // The parser's own span is lines[task.line-1 .. task.own_end_line) (0-based),
    // matching `own_excerpt`; splice that out and insert the new body.
    let lines: Vec<&str> = current.lines().collect();
    let start = task.line.saturating_sub(1);
    let end = task.own_end_line.min(lines.len());
    let mut out = String::new();
    for line in &lines[..start] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&new_body);
    for line in &lines[end..] {
        out.push_str(line);
        out.push('\n');
    }
    let new_doc = Document::parse(&out);
    if new_doc.has_errors() {
        return Err(lint_errors(&new_doc));
    }
    Ok(out)
}

/// `ralph backlog <add|edit> ...` — the flag-shaped aliases ralphd still calls.
/// They queue like every other mutation, so no CLI path writes the backlog in
/// place.
pub fn run(args: &[String]) -> R<i32> {
    let sub = args.first().map(String::as_str);
    let rest = args.get(1..).unwrap_or(&[]);
    let cfg = crate::config::load_base(rest)?;
    let current = crate::backlog_cli::current(&cfg.backlog)?;
    let req = match sub {
        Some("add") => Request::Add {
            id: None,
            under: None,
            title: flag(rest, "--title")
                .ok_or("backlog add: --title <text> required")?
                .to_string(),
            body: verify_body(
                flag(rest, "--verify").ok_or("backlog add: --verify <cmd> required")?,
            ),
        },
        Some("edit") => Request::Edit {
            id: flag(rest, "--id")
                .ok_or("backlog edit: --id <id> required")?
                .to_string(),
            title: flag(rest, "--title")
                .ok_or("backlog edit: --title <text> required")?
                .to_string(),
            verify: flag(rest, "--verify")
                .ok_or("backlog edit: --verify <cmd> required")?
                .to_string(),
        },
        other => return Err(format!("backlog: expected `add` or `edit`, got {other:?}").into()),
    };
    crate::backlog_cli::submit(&cfg, &current, req, "backlog")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backlog::SCHEMA_MARKER;

    #[test]
    fn add_onto_empty_backlog_bootstraps_a_valid_arc() {
        // The exact new-arc path: completion archived BACKLOG.md away, and the
        // first `ralph add` must produce a valid, routable backlog from nothing.
        let (new_text, id) = apply_add_top(
            &empty_backlog(),
            "First of new arc",
            &verify_body("cargo test"),
        )
        .unwrap();
        assert_eq!(id, "1");
        let doc = Document::parse(&new_text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        assert_eq!(
            doc.tasks[doc.selected_index().unwrap()].title,
            "First of new arc"
        );
    }

    #[test]
    fn add_appends_valid_task_with_incremented_id() {
        let current = format!("{SCHEMA_MARKER}\n# B\n- [ ] **1 — First.** Verify: y\n");
        let (new_text, id) =
            apply_add_top(&current, "Second thing", &verify_body("cargo test")).unwrap();
        assert_eq!(id, "2");
        let doc = Document::parse(&new_text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        assert_eq!(doc.tasks.len(), 2);
        assert_eq!(doc.tasks[1].title, "Second thing");
    }

    #[test]
    fn add_rejects_placeholder_verify_without_touching_input() {
        // A marked v1 backlog requires a real Verify; "TODO" is a placeholder.
        let current = format!("{SCHEMA_MARKER}\n# B\n- [ ] **1 — First.** Verify: y\n");
        let err = apply_add_top(&current, "Bad", &verify_body("TODO")).unwrap_err();
        assert!(err.contains("Verify"), "{err}");
    }

    #[test]
    fn add_rejects_double_asterisk_in_title() {
        let current = format!("{SCHEMA_MARKER}\n# B\n- [ ] **1 — First.** Verify: y\n");
        let err =
            apply_add_top(&current, "Support **bold**", &verify_body("cargo test")).unwrap_err();
        assert!(err.contains("**"), "{err}");
    }

    #[test]
    fn add_rejects_title_that_breaks_the_label() {
        let current = format!("{SCHEMA_MARKER}\n# B\n- [ ] **1 — First.** Verify: y\n");
        // An embedded newline splits the bold label across lines, so the
        // opening `**` never finds a closing `**` on the same line → parse error.
        let err = apply_add_top(&current, "Bad\ntitle", &verify_body("cargo test")).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn edit_replaces_title_and_verify_preserving_children() {
        let current = format!(
            "{SCHEMA_MARKER}\n# B\n- [ ] **1 — Parent.**\n  Verify: broad\n  - [ ] **1.1 — Child.** Verify: focused\n"
        );
        let new_text = apply_edit(&current, "1", "Parent renamed", "new broad").unwrap();
        let doc = Document::parse(&new_text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        let parent = doc.tasks.iter().find(|t| t.id == "1").unwrap();
        assert_eq!(parent.title, "Parent renamed");
        // The child stage is untouched.
        assert!(doc
            .tasks
            .iter()
            .any(|t| t.id == "1.1" && t.title == "Child."));
        assert!(new_text.contains("Verify: new broad"));
    }

    #[test]
    fn edit_preserves_checked_box_and_indent() {
        let current = format!(
            "{SCHEMA_MARKER}\n# B\n- [ ] **1 — P.**\n  Verify: broad\n  - [x] **1.1 — Done child.** Verify: focused\n"
        );
        let new_text = apply_edit(&current, "1.1", "Done child renamed", "focused2").unwrap();
        assert!(new_text.contains("  - [x] **1.1 — Done child renamed**"));
    }

    #[test]
    fn uncheck_reopens_leaf_and_checked_ancestors() {
        let current = format!(
            "{SCHEMA_MARKER}\n# B\n- [x] **1 — Parent.**\n  Verify: broad\n  - [x] **1.1 — Child.** Verify: focused\n- [ ] **2 — Next.** Verify: y\n"
        );
        let new_text = apply_uncheck(&current, "1.1").unwrap();
        let doc = Document::parse(&new_text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        // Both the leaf and its closed parent reopen, so the lint stays green
        // and routing re-selects 1.1.
        assert!(new_text.contains("- [ ] **1 — Parent.**"));
        assert!(new_text.contains("  - [ ] **1.1 — Child.**"));
        assert_eq!(doc.tasks[doc.selected_index().unwrap()].id, "1.1");
    }

    #[test]
    fn uncheck_rejects_pending_or_unknown_task() {
        let current = format!("{SCHEMA_MARKER}\n- [ ] **1 — P.** Verify: y\n");
        assert!(apply_uncheck(&current, "1")
            .unwrap_err()
            .contains("not checked"));
        assert!(apply_uncheck(&current, "9")
            .unwrap_err()
            .contains("no task"));
    }

    #[test]
    fn edit_unknown_id_errors() {
        let current = format!("{SCHEMA_MARKER}\n- [ ] **1 — P.** Verify: y\n");
        let err = apply_edit(&current, "99", "x", "y").unwrap_err();
        assert!(err.contains("99"), "{err}");
    }

    fn staged() -> String {
        format!(
            "{SCHEMA_MARKER}\n# B\n- [ ] **1 — Parent.**\n  Verify: broad\n  - [x] **1.1 — First stage.** Verify: a\n  - [ ] **1.2 — Second stage.** Verify: b\n- [ ] **2 — Later.** Verify: y\n"
        )
    }

    #[test]
    fn explicit_id_lands_as_the_last_child_of_its_implied_parent() {
        let (new_text, id) =
            apply_add_with_id(&staged(), "1.3", "Third stage", &verify_body("c")).unwrap();
        assert_eq!(id, "1.3");
        let doc = Document::parse(&new_text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        // Indent is the parent's + 2, and it sits before the next top-level task.
        assert!(new_text.contains("  - [ ] **1.3 — Third stage**\n    Verify: c\n- [ ] **2"));
        let parent = doc.tasks.iter().position(|t| t.id == "1").unwrap();
        assert_eq!(
            doc.tasks.iter().find(|t| t.id == "1.3").unwrap().parent,
            Some(parent)
        );
    }

    #[test]
    fn explicit_id_needs_its_parent_and_refuses_a_duplicate() {
        let err = apply_add_with_id(&staged(), "9.1", "Orphan", &verify_body("c")).unwrap_err();
        assert!(err.contains("`9`"), "{err}");
        let err = apply_add_with_id(&staged(), "1.2", "Clash", &verify_body("c")).unwrap_err();
        assert!(err.contains("already exists (line"), "{err}");
    }

    #[test]
    fn under_numbers_the_next_free_stage() {
        let (_, id) = apply_add_under(&staged(), "1", "Third stage", &verify_body("c")).unwrap();
        assert_eq!(id, "1.3");
        let (_, id) = apply_add_under(&staged(), "2", "First stage", &verify_body("c")).unwrap();
        assert_eq!(id, "2.1");
        assert!(apply_add_under(&staged(), "9", "x", &verify_body("c"))
            .unwrap_err()
            .contains("no task"));
    }

    #[test]
    fn a_piped_body_keeps_its_prose_and_its_contract() {
        let body = "Some constraint.\n\nAnother line.\nVerify: cargo test";
        let (new_text, _) = apply_add_under(&staged(), "1", "With prose", body).unwrap();
        let doc = Document::parse(&new_text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        assert!(
            new_text.contains("    Some constraint.\n\n    Another line.\n    Verify: cargo test"),
            "{new_text}"
        );
    }

    #[test]
    fn done_checks_one_task_and_never_its_parent() {
        let new_text = apply_done(&staged(), "1.2").unwrap();
        let doc = Document::parse(&new_text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        assert!(new_text.contains("  - [x] **1.2 — Second stage.**"));
        // The parent stays open: it is now its own integration step.
        assert!(new_text.contains("- [ ] **1 — Parent.**"));
        assert_eq!(doc.tasks[doc.selected_index().unwrap()].id, "1");
    }

    #[test]
    fn done_rejects_a_parent_with_pending_stages() {
        // The existing "checked parent contains an unchecked stage" lint is the
        // right answer here, not a cascade.
        let err = apply_done(&staged(), "1").unwrap_err();
        assert!(err.contains("unchecked stage"), "{err}");
        assert!(apply_done(&staged(), "1.1")
            .unwrap_err()
            .contains("already checked"));
        assert!(apply_done(&staged(), "9").unwrap_err().contains("no task"));
    }

    #[test]
    fn drop_takes_the_subtree_and_hands_it_back_for_archiving() {
        let (new_text, removed) = apply_drop(&staged(), "1", true).unwrap();
        let doc = Document::parse(&new_text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        assert_eq!(doc.tasks.len(), 1);
        assert_eq!(doc.tasks[0].id, "2");
        assert!(removed.contains("**1 — Parent.**"));
        assert!(removed.contains("**1.2 — Second stage.**"));
    }

    #[test]
    fn drop_refuses_a_parent_without_recursive_and_an_unknown_id() {
        let err = apply_drop(&staged(), "1", false).unwrap_err();
        assert!(err.contains("--recursive"), "{err}");
        assert!(apply_drop(&staged(), "9", true)
            .unwrap_err()
            .contains("no task"));
    }

    #[test]
    fn drop_of_a_leaf_leaves_its_siblings_intact() {
        let (new_text, removed) = apply_drop(&staged(), "1.2", false).unwrap();
        let doc = Document::parse(&new_text);
        assert!(!doc.has_errors(), "{:?}", doc.issues);
        assert!(doc.tasks.iter().any(|t| t.id == "1.1"));
        assert!(!doc.tasks.iter().any(|t| t.id == "1.2"));
        assert_eq!(removed.lines().count(), 1);
    }
}
