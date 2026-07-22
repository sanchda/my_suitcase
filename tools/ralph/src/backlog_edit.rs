//! `ralph backlog add|edit` — schema-safe backlog mutation. Both operations are
//! pure `String -> Result<String, String>` transforms gated by an in-memory
//! lint: an edit that would make the backlog invalid is REJECTED and never
//! reaches disk, so a mutation can never crash a running loop (which aborts on
//! an invalid backlog). Writes are atomic (temp file + rename).

use crate::backlog::{Document, Severity};
use crate::R;

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

/// Append a well-formed top-level task; returns `(new_text, new_id)` or the lint
/// errors that would result.
pub fn apply_add(current: &str, title: &str, verify: &str) -> Result<(String, String), String> {
    if title.contains("**") {
        return Err("task title may not contain `**` (it breaks the bold label)".to_string());
    }
    let doc = Document::parse(current);
    let id = next_top_level_id(&doc);
    let mut text = current.trim_end().to_string();
    text.push('\n');
    text.push_str(&format!(
        "- [ ] **{id} — {}**\n  Verify: {}\n",
        title.trim(),
        verify.trim()
    ));
    let new_doc = Document::parse(&text);
    if new_doc.has_errors() {
        return Err(lint_errors(&new_doc));
    }
    Ok((text, id))
}

/// Atomically replace `path`'s contents with `new_text` (temp file + rename in
/// the same directory, so a reader never sees a half-written backlog).
fn write_atomic(path: &std::path::Path, new_text: &str) -> R<()> {
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("BACKLOG.md"),
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

/// `ralph backlog <add|edit> ...`. Returns exit 0 on success, 1 on a rejected
/// (invalid-result) mutation, and errors on bad usage / IO.
pub fn run(args: &[String]) -> R<i32> {
    let sub = args.first().map(String::as_str);
    let rest = args.get(1..).unwrap_or(&[]);
    let cfg = crate::config::load_base(rest)?;
    let current = std::fs::read_to_string(&cfg.backlog)
        .map_err(|e| format!("{}: cannot read backlog: {e}", cfg.backlog.display()))?;
    match sub {
        Some("add") => {
            let title = flag(rest, "--title").ok_or("backlog add: --title <text> required")?;
            let verify = flag(rest, "--verify").ok_or("backlog add: --verify <cmd> required")?;
            match apply_add(&current, title, verify) {
                Ok((new_text, id)) => {
                    write_atomic(&cfg.backlog, &new_text)?;
                    println!("added task {id}");
                    Ok(0)
                }
                Err(errors) => {
                    eprintln!("backlog add rejected — result would be invalid:\n{errors}");
                    Ok(1)
                }
            }
        }
        Some("edit") => {
            let id = flag(rest, "--id").ok_or("backlog edit: --id <id> required")?;
            let title = flag(rest, "--title").ok_or("backlog edit: --title <text> required")?;
            let verify = flag(rest, "--verify").ok_or("backlog edit: --verify <cmd> required")?;
            match apply_edit(&current, id, title, verify) {
                Ok(new_text) => {
                    write_atomic(&cfg.backlog, &new_text)?;
                    println!("edited task {id}");
                    Ok(0)
                }
                Err(errors) => {
                    eprintln!("backlog edit rejected:\n{errors}");
                    Ok(1)
                }
            }
        }
        other => Err(format!("backlog: expected `add` or `edit`, got {other:?}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backlog::SCHEMA_MARKER;

    #[test]
    fn add_appends_valid_task_with_incremented_id() {
        let current = format!("{SCHEMA_MARKER}\n# B\n- [ ] **1 — First.** Verify: y\n");
        let (new_text, id) = apply_add(&current, "Second thing", "cargo test").unwrap();
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
        let err = apply_add(&current, "Bad", "TODO").unwrap_err();
        assert!(err.contains("Verify"), "{err}");
    }

    #[test]
    fn add_rejects_double_asterisk_in_title() {
        let current = format!("{SCHEMA_MARKER}\n# B\n- [ ] **1 — First.** Verify: y\n");
        let err = apply_add(&current, "Support **bold**", "cargo test").unwrap_err();
        assert!(err.contains("**"), "{err}");
    }

    #[test]
    fn add_rejects_title_that_breaks_the_label() {
        let current = format!("{SCHEMA_MARKER}\n# B\n- [ ] **1 — First.** Verify: y\n");
        // An embedded newline splits the bold label across lines, so the
        // opening `**` never finds a closing `**` on the same line → parse error.
        let err = apply_add(&current, "Bad\ntitle", "cargo test").unwrap_err();
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
        assert!(doc.tasks.iter().any(|t| t.id == "1.1" && t.title == "Child."));
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
    fn edit_unknown_id_errors() {
        let current = format!("{SCHEMA_MARKER}\n- [ ] **1 — P.** Verify: y\n");
        let err = apply_edit(&current, "99", "x", "y").unwrap_err();
        assert!(err.contains("99"), "{err}");
    }
}
