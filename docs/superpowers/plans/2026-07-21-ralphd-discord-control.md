# ralphd — Discord control bridge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a single-tenant Discord bot (`ralphd`) that drives a `ralph` autonomous loop via native slash commands, backed by a few additive `ralph` subcommands with a lint-or-revert safety gate.

**Architecture:** Two binaries. `ralph` (existing, synchronous) gains additive subcommands `ralph status --json`, `ralph backlog add`, `ralph backlog edit`, all early-returning like `init`/`schema` and all backlog mutations gated by an in-memory lint. `ralphd` is a new, separate async crate (`tools/ralphd/`) using serenity+tokio that shells out to the `ralph` binary and reads/writes `.ralph/` files; it never links ralph's code. Authorization is strict: one guild, one channel, one user.

**Tech Stack:** Rust 2021, `serde`/`serde_json`, `toml`, `libc` (existing ralph deps); `serenity = "0.12"` + `tokio` (new, ralphd only).

**Spec:** `docs/superpowers/specs/2026-07-21-ralphd-discord-control-design.md`

---

## File Structure

**Modified (ralph, `tools/ralph/`):**
- `src/config.rs` — add `load_base()` (defaults←file←env, no arg parsing) and reuse it from `main`.
- `src/main.rs` — dispatch new `status` and `backlog` early-return subcommands; update `USAGE`.
- `src/status.rs` — **new** — `ralph status [--json]`.
- `src/backlog_edit.rs` — **new** — `ralph backlog add|edit` with pure `apply_add`/`apply_edit` + atomic write.

**New crate (`tools/ralphd/`):**
- `Cargo.toml`
- `src/main.rs` — tokio entry; parse config; start serenity client.
- `src/config.rs` — `BotConfig` + `parse()` (splits argv at `--`, env fallbacks).
- `src/auth.rs` — pure `authorized()`.
- `src/model.rs` — pure `validate_tier()`.
- `src/loop_pid.rs` — pidfile write/read/liveness/adoption.
- `src/format.rs` — format `ralph status --json` output into a Discord message.
- `src/ralph.rs` — CLI bridge (shell-out wrappers + loop spawn + MODEL write).
- `src/handler.rs` — serenity `EventHandler`: register guild commands, auth-gate, dispatch, reply.

**Modified (packaging/docs):**
- `personalize/scripts/setup_ralph.sh` — build + install `ralphd` too.
- `tools/ralph/README.md` — document the new subcommands and ralphd.

---

## Task 1: `ralph status --json` (+ config::load_base refactor)

**Files:**
- Modify: `tools/ralph/src/config.rs`
- Modify: `tools/ralph/src/main.rs`
- Create: `tools/ralph/src/status.rs`

- [ ] **Step 1: Add a failing test for `config::load_base`**

In `tools/ralph/src/config.rs`, inside the existing `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn load_base_applies_defaults_without_arg_parsing() {
    // No config file, no env: pure defaults, and unknown flags are ignored
    // (load_base does not parse CLI flags — that's apply_args' job).
    let cfg = load_base(&["--json".to_string()]).unwrap();
    assert_eq!(cfg.model, "sonnet");
    assert_eq!(cfg.dir.to_string_lossy(), ".ralph");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --manifest-path tools/ralph/Cargo.toml load_base_applies_defaults -- --nocapture`
Expected: FAIL — `cannot find function 'load_base'`.

- [ ] **Step 3: Implement `load_base` and refactor `main`**

In `tools/ralph/src/config.rs`, add (near the other `pub fn`s):

```rust
/// Resolve config with the precedence prefix **defaults ← file ← env**, WITHOUT
/// applying CLI flags. Subcommands with their own flag grammar (`status`,
/// `backlog`) use this to get the driving-file paths; `main` layers `apply_args`
/// on top for the loop itself.
pub fn load_base(args: &[String]) -> crate::R<Config> {
    let cpath = config_path(args, |k| std::env::var(k).ok());
    let mut cfg = Config::default();
    if cpath.exists() {
        let text = std::fs::read_to_string(&cpath)?;
        let file: FileConfig =
            toml::from_str(&text).map_err(|e| format!("parsing {}: {e}", cpath.display()))?;
        apply_file(&mut cfg, file)?;
    }
    apply_env(&mut cfg, |k| std::env::var(k).ok())?;
    Ok(cfg)
}
```

Then in `tools/ralph/src/main.rs`, replace the inline resolution (the block from `let cpath = config::config_path(...)` through `config::apply_env(...)?;`, currently lines ~96–104) with:

```rust
    let mut cfg = config::load_base(args)?;
    if config::apply_args(&mut cfg, args)? {
        print!("{USAGE}");
        return Ok(0);
    }
    config::validate(&cfg)?;
```

- [ ] **Step 4: Run to verify the refactor test passes**

Run: `cargo test --manifest-path tools/ralph/Cargo.toml load_base_applies_defaults`
Expected: PASS. Also run `cargo test --manifest-path tools/ralph/Cargo.toml` — all existing tests still PASS.

- [ ] **Step 5: Add a failing test for the status report**

Create `tools/ralph/src/status.rs` with only the test module first:

```rust
//! `ralph status [--json]` — a machine- and human-readable snapshot of the loop's
//! backlog frontier. Read-only; run-state (is a loop alive?) is the caller's job.

use crate::backlog::Document;
use crate::config::Config;
use crate::state::State;
use crate::R;
use serde::Serialize;

const EXCERPT_BYTES: usize = 1200;

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
        assert_eq!(report.upcoming, vec!["3 — Next.".to_string(), "4 — After.".to_string()]);
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
```

- [ ] **Step 6: Run to verify it fails**

Run: `cargo test --manifest-path tools/ralph/Cargo.toml report_reflects_selected_leaf`
Expected: FAIL — `cannot find function 'build_report'`.

- [ ] **Step 7: Implement `build_report` and the `run` entrypoint**

Append to `tools/ralph/src/status.rs` (above the test module):

```rust
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
    let mut upcoming = doc.upcoming_leaf_labels(4);
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
    let rest: Vec<String> = args.iter().filter(|a| a.as_str() != "--json").cloned().collect();
    let cfg: Config = crate::config::load_base(&rest)?;
    let iteration = State::open(&cfg.dir).map(|s| s.iteration()).unwrap_or(0);
    let text = std::fs::read_to_string(&cfg.backlog)
        .map_err(|e| format!("{}: cannot read backlog: {e}", cfg.backlog.display()))?;
    let doc = Document::parse(&text);
    if doc.has_errors() {
        return Err(format!(
            "backlog schema is invalid; run `ralph lint` for details"
        )
        .into());
    }
    let report = build_report(&doc, iteration);
    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        match &report.current {
            Some(c) => println!("iter {} · {} pending · current: {}", report.iteration, report.pending_leaf_count, c.label),
            None => println!("iter {} · backlog complete", report.iteration),
        }
        for label in &report.upcoming {
            println!("  next: {label}");
        }
    }
    Ok(0)
}
```

- [ ] **Step 8: Wire the subcommand into `main`**

In `tools/ralph/src/main.rs`: add `mod status;` alongside the other `mod` lines, and add this early-return block right after the existing `schema` dispatch (near line 84):

```rust
    if argv.first().map(String::as_str) == Some("status") {
        return status::run(&argv[1..]);
    }
```

- [ ] **Step 9: Run tests**

Run: `cargo test --manifest-path tools/ralph/Cargo.toml`
Expected: PASS (all, including the two new status tests).

- [ ] **Step 10: Manually verify the JSON shape**

Run: `cargo run --manifest-path tools/ralph/Cargo.toml -- status --json` from a repo that has a `.ralph/BACKLOG.md` (or `cd` into `tools/ralph` and point at a fixture). 
Expected: one line of JSON with keys `iteration`, `pending_leaf_count`, `current`, `upcoming`.

- [ ] **Step 11: Commit**

```bash
git add tools/ralph/src/config.rs tools/ralph/src/main.rs tools/ralph/src/status.rs
git commit -m "ralph: add 'status --json' subcommand and config::load_base"
```

---

## Task 2: `ralph backlog add`

**Files:**
- Create: `tools/ralph/src/backlog_edit.rs`
- Modify: `tools/ralph/src/main.rs`

- [ ] **Step 1: Write failing tests for `apply_add`**

Create `tools/ralph/src/backlog_edit.rs`:

```rust
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
    fn add_rejects_title_that_breaks_the_label() {
        let current = format!("{SCHEMA_MARKER}\n# B\n- [ ] **1 — First.** Verify: y\n");
        // A `**` inside the title closes the bold label early → parse error.
        let err = apply_add(&current, "has ** inside", "cargo test").unwrap_err();
        assert!(!err.is_empty());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --manifest-path tools/ralph/Cargo.toml add_appends_valid`
Expected: FAIL — module `backlog_edit` not declared.

- [ ] **Step 3: Declare the module**

In `tools/ralph/src/main.rs` add `mod backlog_edit;` alongside the other `mod` lines.

- [ ] **Step 4: Run to verify the add tests pass**

Run: `cargo test --manifest-path tools/ralph/Cargo.toml backlog_edit::tests`
Expected: PASS (all three add tests).

- [ ] **Step 5: Add the `run` entrypoint + atomic write**

Append to `tools/ralph/src/backlog_edit.rs` (above the test module):

```rust
/// Atomically replace `path`'s contents with `new_text` (temp file + rename in
/// the same directory, so a reader never sees a half-written backlog).
fn write_atomic(path: &std::path::Path, new_text: &str) -> R<()> {
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("BACKLOG.md")
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

/// `ralph backlog <add|edit> ...`. Returns exit 0 on success, 1 on a rejected
/// (invalid-result) mutation, and errors on bad usage / IO.
pub fn run(args: &[String]) -> R<i32> {
    let sub = args.first().map(String::as_str);
    let rest = &args[args.len().min(1)..];
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
```

> Note: `apply_edit` is implemented in Task 3; this `run` references it now so the module compiles only after Task 3. To keep this task green in isolation, temporarily add a stub above the tests: `pub fn apply_edit(_c: &str, _i: &str, _t: &str, _v: &str) -> Result<String, String> { Err("unimplemented".into()) }` and replace it in Task 3.

- [ ] **Step 6: Wire the subcommand into `main`**

In `tools/ralph/src/main.rs`, add this early-return after the `status` block:

```rust
    if argv.first().map(String::as_str) == Some("backlog") {
        return backlog_edit::run(&argv[1..]);
    }
```

- [ ] **Step 7: Run tests + a manual add**

Run: `cargo test --manifest-path tools/ralph/Cargo.toml`
Expected: PASS.
Manual: in a scratch repo with a valid `.ralph/BACKLOG.md`, run `cargo run --manifest-path <path>/tools/ralph/Cargo.toml -- backlog add --title "Try it" --verify "cargo test"`; confirm the task is appended and `ralph lint` stays clean.

- [ ] **Step 8: Commit**

```bash
git add tools/ralph/src/backlog_edit.rs tools/ralph/src/main.rs
git commit -m "ralph: add 'backlog add' with lint-or-revert atomic write"
```

---

## Task 3: `ralph backlog edit`

**Files:**
- Modify: `tools/ralph/src/backlog_edit.rs`

- [ ] **Step 1: Write failing tests for `apply_edit`**

In `tools/ralph/src/backlog_edit.rs`, add to `mod tests`:

```rust
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --manifest-path tools/ralph/Cargo.toml edit_replaces_title`
Expected: FAIL — the stub returns `Err("unimplemented")`, so the assertions fail.

- [ ] **Step 3: Replace the stub with the real `apply_edit`**

In `tools/ralph/src/backlog_edit.rs`, remove the temporary stub and add:

```rust
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test --manifest-path tools/ralph/Cargo.toml backlog_edit::tests`
Expected: PASS (all add + edit tests).

- [ ] **Step 5: Full suite**

Run: `cargo test --manifest-path tools/ralph/Cargo.toml`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add tools/ralph/src/backlog_edit.rs
git commit -m "ralph: add 'backlog edit' (own-span replace, children preserved)"
```

---

## Task 4: ralphd crate scaffold + config parsing

**Files:**
- Create: `tools/ralphd/Cargo.toml`
- Create: `tools/ralphd/src/main.rs`
- Create: `tools/ralphd/src/config.rs`

- [ ] **Step 1: Create the crate manifest**

Create `tools/ralphd/Cargo.toml`:

```toml
[package]
name = "ralphd"
version = "0.1.0"
edition = "2021"
description = "Single-tenant Discord control bridge for the ralph autonomous loop."

[[bin]]
name = "ralphd"
path = "src/main.rs"

[dependencies]
serde_json = "1"
libc = "0.2"
serenity = "0.12"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "process"] }

[profile.release]
opt-level = "s"
```

- [ ] **Step 2: Create the config module with a failing test**

Create `tools/ralphd/src/config.rs`:

```rust
//! ralphd configuration: its own settings come from flags before `--` (with
//! env fallbacks); everything after `--` is the fixed loop profile forwarded to
//! `ralph` verbatim on `/start`. The bot token is env-only (`DISCORD_BOT_TOKEN`).

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct BotConfig {
    pub token: String,
    pub guild_id: u64,
    pub channel_id: u64,
    pub user_id: u64,
    pub working_dir: PathBuf,
    pub state_dir: PathBuf,
    pub ralph_args: Vec<String>,
}

/// Parse `(argv, env)` into a `BotConfig`. `argv` excludes the program name.
/// `env` looks up an environment variable by name.
pub fn parse(argv: &[String], env: impl Fn(&str) -> Option<String>) -> Result<BotConfig, String> {
    // Split at the first bare `--`: before = ralphd flags, after = ralph args.
    let split = argv.iter().position(|a| a == "--");
    let (mine, forwarded) = match split {
        Some(i) => (&argv[..i], argv[i + 1..].to_vec()),
        None => (&argv[..], Vec::new()),
    };

    let flag = |name: &str| -> Option<String> {
        mine.iter()
            .position(|a| a == name)
            .and_then(|i| mine.get(i + 1))
            .cloned()
    };

    let token = env("DISCORD_BOT_TOKEN")
        .filter(|t| !t.trim().is_empty())
        .ok_or("DISCORD_BOT_TOKEN is required (env only)")?;

    let want_u64 = |name: &str, env_key: &str| -> Result<u64, String> {
        let raw = flag(name)
            .or_else(|| env(env_key))
            .ok_or_else(|| format!("{name} (or {env_key}) is required"))?;
        raw.trim()
            .parse::<u64>()
            .map_err(|_| format!("{name} must be a numeric Discord id, got `{raw}`"))
    };

    let guild_id = want_u64("--guild", "RALPHD_GUILD_ID")?;
    let channel_id = want_u64("--channel", "RALPHD_CHANNEL_ID")?;
    let user_id = want_u64("--user", "RALPHD_USER_ID")?;

    let working_dir: PathBuf = flag("--working-dir")
        .or_else(|| env("RALPHD_WORKING_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    // The loop's state dir defaults to `<working-dir>/.ralph`, but honor a
    // `--dir <path>` in the forwarded ralph args so /model writes the right file.
    let dir_override = forwarded
        .iter()
        .position(|a| a == "--dir")
        .and_then(|i| forwarded.get(i + 1))
        .map(PathBuf::from);
    let state_dir = dir_override.unwrap_or_else(|| working_dir.join(".ralph"));

    Ok(BotConfig {
        token,
        guild_id,
        channel_id,
        user_id,
        working_dir,
        state_dir,
        ralph_args: forwarded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + '_ {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn parse_splits_flags_from_forwarded_args() {
        let argv: Vec<String> = ["--guild", "1", "--channel", "2", "--user", "3", "--", "--model", "opus", "--dir", "custom"]
            .iter().map(|s| s.to_string()).collect();
        let cfg = parse(&argv, env_map(&[("DISCORD_BOT_TOKEN", "tok")])).unwrap();
        assert_eq!(cfg.guild_id, 1);
        assert_eq!(cfg.channel_id, 2);
        assert_eq!(cfg.user_id, 3);
        assert_eq!(cfg.ralph_args, vec!["--model", "opus", "--dir", "custom"]);
        assert_eq!(cfg.state_dir, PathBuf::from("custom"));
    }

    #[test]
    fn parse_defaults_state_dir_under_working_dir() {
        let argv: Vec<String> = ["--guild", "1", "--channel", "2", "--user", "3", "--working-dir", "/repo"]
            .iter().map(|s| s.to_string()).collect();
        let cfg = parse(&argv, env_map(&[("DISCORD_BOT_TOKEN", "tok")])).unwrap();
        assert_eq!(cfg.state_dir, PathBuf::from("/repo/.ralph"));
        assert!(cfg.ralph_args.is_empty());
    }

    #[test]
    fn parse_requires_token() {
        let argv: Vec<String> = ["--guild", "1", "--channel", "2", "--user", "3"]
            .iter().map(|s| s.to_string()).collect();
        assert!(parse(&argv, env_map(&[])).is_err());
    }

    #[test]
    fn parse_reads_ids_from_env() {
        let argv: Vec<String> = Vec::new();
        let cfg = parse(&argv, env_map(&[
            ("DISCORD_BOT_TOKEN", "tok"),
            ("RALPHD_GUILD_ID", "10"),
            ("RALPHD_CHANNEL_ID", "20"),
            ("RALPHD_USER_ID", "30"),
        ])).unwrap();
        assert_eq!((cfg.guild_id, cfg.channel_id, cfg.user_id), (10, 20, 30));
    }
}
```

- [ ] **Step 3: Create a minimal `main.rs` so the crate builds**

Create `tools/ralphd/src/main.rs`:

```rust
mod config;

fn main() {
    // Real entry is added in Task 8; for now just parse config and report.
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match config::parse(&argv, |k| std::env::var(k).ok()) {
        Ok(cfg) => {
            eprintln!(
                "ralphd: configured (guild={}, channel={}, user={}, dir={})",
                cfg.guild_id, cfg.channel_id, cfg.user_id, cfg.state_dir.display()
            );
        }
        Err(e) => {
            eprintln!("ralphd: {e}");
            std::process::exit(2);
        }
    }
}
```

- [ ] **Step 4: Run the config tests**

Run: `cargo test --manifest-path tools/ralphd/Cargo.toml`
Expected: PASS (four config tests). First build will download serenity/tokio — that's expected.

- [ ] **Step 5: Commit**

```bash
git add tools/ralphd/Cargo.toml tools/ralphd/src/config.rs tools/ralphd/src/main.rs
git commit -m "ralphd: scaffold crate + config parsing (flags/env, arg forwarding)"
```

---

## Task 5: Auth gate, model validation, status formatting

**Files:**
- Create: `tools/ralphd/src/auth.rs`
- Create: `tools/ralphd/src/model.rs`
- Create: `tools/ralphd/src/format.rs`
- Modify: `tools/ralphd/src/main.rs`

- [ ] **Step 1: Write the auth module + test**

Create `tools/ralphd/src/auth.rs`:

```rust
//! The entire ralphd security model: a command is honored only from the one
//! configured channel AND the one configured user. Everything else is refused.

use crate::config::BotConfig;

/// True only when BOTH the channel and the user match the configured single tenant.
pub fn authorized(channel_id: u64, user_id: u64, cfg: &BotConfig) -> bool {
    channel_id == cfg.channel_id && user_id == cfg.user_id
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn cfg() -> BotConfig {
        BotConfig {
            token: "t".into(),
            guild_id: 1,
            channel_id: 100,
            user_id: 200,
            working_dir: PathBuf::from("."),
            state_dir: PathBuf::from(".ralph"),
            ralph_args: vec![],
        }
    }

    #[test]
    fn only_matching_channel_and_user_is_authorized() {
        let c = cfg();
        assert!(authorized(100, 200, &c));
        assert!(!authorized(999, 200, &c)); // wrong channel
        assert!(!authorized(100, 999, &c)); // wrong user
        assert!(!authorized(999, 999, &c));
    }
}
```

- [ ] **Step 2: Write the model module + test**

Create `tools/ralphd/src/model.rs`:

```rust
//! Validation for `/model <tier>` — only the three known tiers are accepted, and
//! the value is canonicalized (trimmed, lowercased) before it is written to
//! `.ralph/MODEL` as a one-shot override for the next iteration.

const TIERS: [&str; 3] = ["haiku", "sonnet", "opus"];

/// Return the canonical tier string if `raw` names a known tier, else `None`.
pub fn validate_tier(raw: &str) -> Option<&'static str> {
    let normalized = raw.trim().to_ascii_lowercase();
    TIERS.iter().copied().find(|t| *t == normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_known_tiers_case_insensitively() {
        assert_eq!(validate_tier("opus"), Some("opus"));
        assert_eq!(validate_tier("  Sonnet "), Some("sonnet"));
        assert_eq!(validate_tier("HAIKU"), Some("haiku"));
    }

    #[test]
    fn rejects_unknown_tiers() {
        assert_eq!(validate_tier("gpt5"), None);
        assert_eq!(validate_tier(""), None);
    }
}
```

- [ ] **Step 3: Write the format module + test**

Create `tools/ralphd/src/format.rs`:

```rust
//! Turn `ralph status --json` output into a compact Discord message. Kept pure
//! (string in, string out) so it is unit-testable without a live gateway.

use serde_json::Value;

/// Format a status JSON document plus a run-state line into a Discord message.
/// `running` is ralphd's own pid-liveness verdict (ralph does not know it).
pub fn status_message(json: &str, running: bool) -> String {
    let v: Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return format!("⚠️ could not parse ralph status output:\n```\n{json}\n```"),
    };
    let iter = v.get("iteration").and_then(Value::as_u64).unwrap_or(0);
    let pending = v.get("pending_leaf_count").and_then(Value::as_u64).unwrap_or(0);
    let run = if running { "▶️ running" } else { "⏸️ idle" };
    let mut out = format!("**ralph** — {run} · iter {iter} · {pending} pending\n");
    match v.get("current") {
        Some(Value::Object(c)) => {
            let label = c.get("label").and_then(Value::as_str).unwrap_or("?");
            out.push_str(&format!("**current:** {label}\n"));
        }
        _ => out.push_str("**current:** backlog complete\n"),
    }
    if let Some(Value::Array(upcoming)) = v.get("upcoming") {
        for item in upcoming {
            if let Some(s) = item.as_str() {
                out.push_str(&format!("• {s}\n"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_a_running_snapshot() {
        let json = r#"{"iteration":7,"pending_leaf_count":3,"current":{"id":"2","label":"2 — Current.","excerpt":"..."},"upcoming":["3 — Next.","4 — After."]}"#;
        let msg = status_message(json, true);
        assert!(msg.contains("running"));
        assert!(msg.contains("iter 7"));
        assert!(msg.contains("3 pending"));
        assert!(msg.contains("2 — Current."));
        assert!(msg.contains("• 3 — Next."));
    }

    #[test]
    fn formats_a_complete_backlog() {
        let json = r#"{"iteration":9,"pending_leaf_count":0,"current":null,"upcoming":[]}"#;
        let msg = status_message(json, false);
        assert!(msg.contains("idle"));
        assert!(msg.contains("backlog complete"));
    }
}
```

- [ ] **Step 4: Declare the modules**

In `tools/ralphd/src/main.rs`, add near the top: `mod auth;`, `mod model;`, `mod format;`.

- [ ] **Step 5: Run tests**

Run: `cargo test --manifest-path tools/ralphd/Cargo.toml`
Expected: PASS (config + auth + model + format tests).

- [ ] **Step 6: Commit**

```bash
git add tools/ralphd/src/auth.rs tools/ralphd/src/model.rs tools/ralphd/src/format.rs tools/ralphd/src/main.rs
git commit -m "ralphd: auth gate, model-tier validation, status formatting"
```

---

## Task 6: Loop pidfile (liveness + adoption)

**Files:**
- Create: `tools/ralphd/src/loop_pid.rs`
- Modify: `tools/ralphd/src/main.rs`

- [ ] **Step 1: Write the module + tests**

Create `tools/ralphd/src/loop_pid.rs`:

```rust
//! Single-loop enforcement via `<state_dir>/loop.pid`. ralphd writes it when it
//! spawns a loop and consults it (with a liveness probe) to refuse a second
//! `/start` and to re-adopt a still-running loop after a ralphd restart.

use std::path::{Path, PathBuf};

fn pidfile(state_dir: &Path) -> PathBuf {
    state_dir.join("loop.pid")
}

/// Is a process with this pid alive? `kill(pid, 0)` performs the permission/
/// existence check without sending a signal.
pub fn is_alive(pid: u32) -> bool {
    // Signal 0: no-op delivery, but errors with ESRCH if the pid is gone.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Record the running loop's pid.
pub fn write(state_dir: &Path, pid: u32) -> std::io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(pidfile(state_dir), format!("{pid}\n"))
}

/// Read the recorded pid, if any and parseable.
pub fn read(state_dir: &Path) -> Option<u32> {
    std::fs::read_to_string(pidfile(state_dir))
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// The pid of a live loop, if one is recorded and still alive. A recorded but
/// dead pid is stale — it is removed and `None` returned.
pub fn running(state_dir: &Path) -> Option<u32> {
    match read(state_dir) {
        Some(pid) if is_alive(pid) => Some(pid),
        Some(_) => {
            let _ = std::fs::remove_file(pidfile(state_dir));
            None
        }
        None => None,
    }
}

/// Clear the pidfile (best-effort), e.g. after a confirmed stop.
pub fn clear(state_dir: &Path) {
    let _ = std::fs::remove_file(pidfile(state_dir));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "ralphd-pid-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn current_process_is_alive_bogus_pid_is_not() {
        assert!(is_alive(std::process::id()));
        // Very high pid unlikely to exist.
        assert!(!is_alive(4_000_000_000));
    }

    #[test]
    fn running_returns_live_pid_and_clears_stale() {
        let dir = tmp();
        // A live pid (our own) is reported as running.
        write(&dir, std::process::id()).unwrap();
        assert_eq!(running(&dir), Some(std::process::id()));
        // A dead pid is treated as stale and removed.
        write(&dir, 4_000_000_000).unwrap();
        assert_eq!(running(&dir), None);
        assert_eq!(read(&dir), None);
    }
}
```

- [ ] **Step 2: Declare the module**

In `tools/ralphd/src/main.rs`, add `mod loop_pid;`.

- [ ] **Step 3: Run tests**

Run: `cargo test --manifest-path tools/ralphd/Cargo.toml loop_pid`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add tools/ralphd/src/loop_pid.rs tools/ralphd/src/main.rs
git commit -m "ralphd: loop pidfile with liveness probe and stale-clearing"
```

---

## Task 7: Ralph CLI bridge

**Files:**
- Create: `tools/ralphd/src/ralph.rs`
- Modify: `tools/ralphd/src/main.rs`

- [ ] **Step 1: Write the bridge + a construction test**

Create `tools/ralphd/src/ralph.rs`:

```rust
//! The bridge from ralphd to the `ralph` binary and `.ralph/` files. All
//! blocking `ralph` calls here are fast (lint/status/backlog run in ms); only
//! `spawn_loop` starts a long-lived process, which we do NOT wait on.

use crate::config::BotConfig;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

pub struct Ralph {
    working_dir: PathBuf,
    state_dir: PathBuf,
    ralph_args: Vec<String>,
}

/// Result of a short `ralph` invocation.
pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Ralph {
    pub fn new(cfg: &BotConfig) -> Self {
        Ralph {
            working_dir: cfg.working_dir.clone(),
            state_dir: cfg.state_dir.clone(),
            ralph_args: cfg.ralph_args.clone(),
        }
    }

    fn run(&self, args: &[&str]) -> std::io::Result<Output> {
        let out = Command::new("ralph")
            .args(args)
            .current_dir(&self.working_dir)
            .output()?;
        Ok(Output {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    /// `ralph status --json` → the raw JSON line on stdout.
    pub fn status_json(&self) -> std::io::Result<Output> {
        self.run(&["status", "--json"])
    }

    /// `ralph stop` — graceful halt after the current iteration.
    pub fn stop(&self) -> std::io::Result<Output> {
        self.run(&["stop"])
    }

    pub fn backlog_add(&self, title: &str, verify: &str) -> std::io::Result<Output> {
        self.run(&["backlog", "add", "--title", title, "--verify", verify])
    }

    pub fn backlog_edit(&self, id: &str, title: &str, verify: &str) -> std::io::Result<Output> {
        self.run(&["backlog", "edit", "--id", id, "--title", title, "--verify", verify])
    }

    /// Write the one-shot `.ralph/MODEL` override. `tier` MUST already be
    /// validated (see `model::validate_tier`).
    pub fn write_model(&self, tier: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.state_dir)?;
        std::fs::write(self.state_dir.join("MODEL"), format!("{tier}\n"))
    }

    /// Spawn the loop: `ralph <forwarded args>` in the working dir. The child is
    /// detached from stdio and NOT waited on; the caller records its pid.
    pub fn spawn_loop(&self) -> std::io::Result<Child> {
        Command::new("ralph")
            .args(&self.ralph_args)
            .current_dir(&self.working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_captures_config_paths() {
        let cfg = BotConfig {
            token: "t".into(),
            guild_id: 1,
            channel_id: 2,
            user_id: 3,
            working_dir: PathBuf::from("/repo"),
            state_dir: PathBuf::from("/repo/.ralph"),
            ralph_args: vec!["--model".into(), "opus".into()],
        };
        let r = Ralph::new(&cfg);
        assert_eq!(r.working_dir, PathBuf::from("/repo"));
        assert_eq!(r.state_dir, PathBuf::from("/repo/.ralph"));
        assert_eq!(r.ralph_args, vec!["--model".to_string(), "opus".to_string()]);
    }

    #[test]
    fn write_model_creates_the_file() {
        let dir = std::env::temp_dir().join(format!("ralphd-model-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = BotConfig {
            token: "t".into(), guild_id: 1, channel_id: 2, user_id: 3,
            working_dir: dir.clone(), state_dir: dir.clone(), ralph_args: vec![],
        };
        Ralph::new(&cfg).write_model("opus").unwrap();
        let written = std::fs::read_to_string(dir.join("MODEL")).unwrap();
        assert_eq!(written.trim(), "opus");
    }
}
```

- [ ] **Step 2: Declare the module**

In `tools/ralphd/src/main.rs`, add `mod ralph;`.

- [ ] **Step 3: Run tests**

Run: `cargo test --manifest-path tools/ralphd/Cargo.toml ralph::tests`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add tools/ralphd/src/ralph.rs tools/ralphd/src/main.rs
git commit -m "ralphd: ralph CLI bridge (status/stop/backlog/model/spawn)"
```

---

## Task 8: Serenity handler + main (gateway wiring)

**Files:**
- Create: `tools/ralphd/src/handler.rs`
- Modify: `tools/ralphd/src/main.rs`

> **Library note:** serenity's builder/interaction API differs across versions. Before writing this task, fetch the serenity 0.12 docs via context7 (`resolve-library-id` → `serenity`, then `query-docs` for "slash commands guild set_commands interaction_create create_response"). The code below is the intended structure; adjust type/method names to the resolved 0.12 API. This task is verified manually against a real bot (no unit tests for the gateway itself — the logic it calls is already tested in Tasks 5–7).

- [ ] **Step 1: Implement the handler**

Create `tools/ralphd/src/handler.rs`:

```rust
//! serenity event handler: register the guild slash commands on ready, then on
//! each command interaction enforce the single-tenant auth gate and dispatch to
//! the ralph bridge. Replies are normal channel messages (a shared audit trail);
//! auth rejections are ephemeral.

use crate::config::BotConfig;
use crate::ralph::Ralph;
use crate::{auth, format, loop_pid, model};
use serenity::async_trait;
use serenity::builder::{
    CreateCommand, CreateCommandOption, CreateInteractionResponse,
    CreateInteractionResponseMessage,
};
use serenity::model::application::{CommandOptionType, Interaction};
use serenity::model::gateway::Ready;
use serenity::model::id::GuildId;
use serenity::prelude::*;

pub struct Handler {
    pub cfg: BotConfig,
}

impl Handler {
    fn ralph(&self) -> Ralph {
        Ralph::new(&self.cfg)
    }

    /// Produce the reply text for a dispatched, already-authorized command.
    fn dispatch(&self, name: &str, opt: |&str| -> Option<String>) -> String {
        // NOTE: `opt` is pseudo-code for "get a string option by name"; wire it
        // to the interaction's resolved options in `interaction_create` below.
        let r = self.ralph();
        match name {
            "start" => {
                if let Some(pid) = loop_pid::running(&self.cfg.state_dir) {
                    return format!("already running (pid {pid})");
                }
                match r.spawn_loop() {
                    Ok(child) => {
                        let pid = child.id();
                        let _ = loop_pid::write(&self.cfg.state_dir, pid);
                        format!("started ralph (pid {pid})")
                    }
                    Err(e) => format!("failed to start: {e}"),
                }
            }
            "stop" => match r.stop() {
                Ok(o) if o.ok => "stop requested — halts after the current iteration".into(),
                Ok(o) => format!("stop failed: {}", o.stderr.trim()),
                Err(e) => format!("stop failed: {e}"),
            },
            "model" => {
                let raw = opt("tier").unwrap_or_default();
                match model::validate_tier(&raw) {
                    Some(tier) => match r.write_model(tier) {
                        Ok(()) => format!("next iteration → {tier} (one-shot)"),
                        Err(e) => format!("could not write MODEL: {e}"),
                    },
                    None => format!("unknown tier `{raw}` (use haiku, sonnet, or opus)"),
                }
            }
            "status" | "next" => match r.status_json() {
                Ok(o) if o.ok => {
                    let running = loop_pid::running(&self.cfg.state_dir).is_some();
                    format::status_message(&o.stdout, running)
                }
                Ok(o) => format!("status failed: {}", o.stderr.trim()),
                Err(e) => format!("status failed: {e}"),
            },
            "backlog-add" => {
                let title = opt("title").unwrap_or_default();
                let verify = opt("verify").unwrap_or_default();
                match r.backlog_add(&title, &verify) {
                    Ok(o) if o.ok => o.stdout.trim().to_string(),
                    Ok(o) => format!("rejected: {}", o.stderr.trim()),
                    Err(e) => format!("backlog add failed: {e}"),
                }
            }
            "backlog-edit" => {
                let id = opt("id").unwrap_or_default();
                let title = opt("title").unwrap_or_default();
                let verify = opt("verify").unwrap_or_default();
                match r.backlog_edit(&id, &title, &verify) {
                    Ok(o) if o.ok => o.stdout.trim().to_string(),
                    Ok(o) => format!("rejected: {}", o.stderr.trim()),
                    Err(e) => format!("backlog edit failed: {e}"),
                }
            }
            other => format!("unknown command `{other}`"),
        }
    }
}

/// The guild slash commands ralphd registers.
fn commands() -> Vec<CreateCommand> {
    let tier = CreateCommandOption::new(CommandOptionType::String, "tier", "haiku, sonnet, or opus")
        .required(true);
    vec![
        CreateCommand::new("start").description("Start the ralph loop"),
        CreateCommand::new("stop").description("Gracefully stop after the current iteration"),
        CreateCommand::new("model").description("One-shot model override for the next iteration").add_option(tier),
        CreateCommand::new("status").description("Loop status: iteration, pending count, current + next tasks"),
        CreateCommand::new("next").description("Show the current and upcoming backlog tasks"),
        CreateCommand::new("backlog-add")
            .description("Append a backlog task (validated before saving)")
            .add_option(CreateCommandOption::new(CommandOptionType::String, "title", "Task title").required(true))
            .add_option(CreateCommandOption::new(CommandOptionType::String, "verify", "Verify command").required(true)),
        CreateCommand::new("backlog-edit")
            .description("Edit a backlog task's title and verify (validated before saving)")
            .add_option(CreateCommandOption::new(CommandOptionType::String, "id", "Task id").required(true))
            .add_option(CreateCommandOption::new(CommandOptionType::String, "title", "New title").required(true))
            .add_option(CreateCommandOption::new(CommandOptionType::String, "verify", "New verify command").required(true)),
    ]
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        let guild = GuildId::new(self.cfg.guild_id);
        if let Err(e) = guild.set_commands(&ctx.http, commands()).await {
            eprintln!("ralphd: failed to register guild commands: {e}");
        } else {
            eprintln!("ralphd: ready as {} — commands registered to guild {}", ready.user.name, self.cfg.guild_id);
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Interaction::Command(command) = interaction else { return };

        let channel_id = command.channel_id.get();
        let user_id = command.user.id.get();
        if !auth::authorized(channel_id, user_id, &self.cfg) {
            let deny = CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .ephemeral(true)
                    .content("not authorized in this channel"),
            );
            let _ = command.create_response(&ctx.http, deny).await;
            return;
        }

        // Resolve a string option by name from this interaction.
        let get = |name: &str| -> Option<String> {
            command
                .data
                .options
                .iter()
                .find(|o| o.name == name)
                .and_then(|o| o.value.as_str().map(str::to_string))
        };

        let reply = self.dispatch(command.data.name.as_str(), get);
        let response = CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new().content(reply),
        );
        if let Err(e) = command.create_response(&ctx.http, response).await {
            eprintln!("ralphd: failed to reply: {e}");
        }
    }
}
```

> The `dispatch` signature uses a closure param; Rust cannot express `|&str| -> Option<String>` as a bare type. Implement `dispatch` as a generic method: `fn dispatch(&self, name: &str, opt: impl Fn(&str) -> Option<String>) -> String`. Update the signature accordingly when writing the file.

- [ ] **Step 2: Replace `main.rs` with the real entry**

Replace `tools/ralphd/src/main.rs` body (keep all the `mod` declarations, add `mod handler;`) with:

```rust
mod auth;
mod config;
mod format;
mod handler;
mod loop_pid;
mod model;
mod ralph;

use serenity::prelude::*;

#[tokio::main]
async fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cfg = match config::parse(&argv, |k| std::env::var(k).ok()) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("ralphd: {e}");
            eprintln!("usage: DISCORD_BOT_TOKEN=… ralphd --guild <id> --channel <id> --user <id> [--working-dir <path>] -- <ralph args…>");
            std::process::exit(2);
        }
    };

    // GUILD_MESSAGES intent is not needed for slash commands; the default
    // (non-privileged) intents suffice for application-command interactions.
    let intents = GatewayIntents::empty();
    let token = cfg.token.clone();
    let mut client = match Client::builder(&token, intents)
        .event_handler(handler::Handler { cfg })
        .await
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ralphd: could not build client: {e}");
            std::process::exit(1);
        }
    };

    eprintln!("ralphd: connecting…");
    if let Err(e) = client.start().await {
        eprintln!("ralphd: client error: {e}");
        std::process::exit(1);
    }
}
```

- [ ] **Step 3: Build**

Run: `cargo build --manifest-path tools/ralphd/Cargo.toml`
Expected: compiles. Fix any 0.12 API mismatches using the context7 docs (method/type names on the builders, `command.data.options` value access).

- [ ] **Step 4: Run the full ralphd test suite**

Run: `cargo test --manifest-path tools/ralphd/Cargo.toml`
Expected: PASS (config/auth/model/format/loop_pid/ralph tests).

- [ ] **Step 5: Manual gateway smoke test**

Create a Discord application + bot; invite it to a test guild with `bot` + `applications.commands` scopes. Then:

```bash
DISCORD_BOT_TOKEN=<token> cargo run --manifest-path tools/ralphd/Cargo.toml -- \
  --guild <GUILD_ID> --channel <CHANNEL_ID> --user <YOUR_USER_ID> \
  --working-dir <a repo with .ralph> -- --max-iterations 5
```

Expected: "ready … commands registered". In the configured channel as the configured user, `/status` returns a snapshot; `/start` reports a pid; `/stop` acknowledges; `/model opus` confirms one-shot; `/backlog-add` appends and `ralph lint` stays clean. From another channel or user, commands are refused.

- [ ] **Step 6: Commit**

```bash
git add tools/ralphd/src/handler.rs tools/ralphd/src/main.rs
git commit -m "ralphd: serenity gateway wiring — guild slash commands, auth gate, dispatch"
```

---

## Task 9: Build/install + docs

**Files:**
- Modify: `personalize/scripts/setup_ralph.sh`
- Modify: `tools/ralph/README.md`

- [ ] **Step 1: Extend the install script to build + install ralphd**

In `personalize/scripts/setup_ralph.sh`, after the existing ralph install block (after `echo "Installed: $BIN"`), add:

```bash
# --- ralphd (optional Discord control bridge) ---
RALPHD_DIR="$SUITCASE_ROOT/tools/ralphd"
if [ -f "$RALPHD_DIR/Cargo.toml" ]; then
  echo "Building ralphd (release)..."
  cargo build --release --manifest-path "$RALPHD_DIR/Cargo.toml"
  RALPHD_BIN="$BIN_DIR/ralphd"
  RALPHD_TMP="$(mktemp "$BIN_DIR/.ralphd.XXXXXX")"
  trap 'rm -f "$RALPHD_TMP"' EXIT
  cp "$RALPHD_DIR/target/release/ralphd" "$RALPHD_TMP"
  chmod 755 "$RALPHD_TMP"
  mv -f "$RALPHD_TMP" "$RALPHD_BIN"
  trap - EXIT
  echo "Installed: $RALPHD_BIN"
fi
```

- [ ] **Step 2: Verify the install script runs**

Run: `bash personalize/scripts/setup_ralph.sh`
Expected: builds both crates, prints `Installed: …/ralph` and `Installed: …/ralphd`.

- [ ] **Step 3: Document the new subcommands + ralphd in the README**

In `tools/ralph/README.md`, add a section after "Watching / controlling a running loop":

```markdown
## Query & edit from the CLI
- `ralph status [--json]` — a snapshot of the backlog frontier: iteration,
  pending-leaf count, the current selected task, and the next few upcoming
  tasks. `--json` emits one machine-readable line.
- `ralph backlog add --title "<t>" --verify "<cmd>"` — append a well-formed task
  (auto-assigned top-level id). Rejected without touching the file if the result
  would fail schema lint.
- `ralph backlog edit --id <id> --title "<t>" --verify "<cmd>"` — replace a
  task's title and verify in place (children preserved). Same lint-or-revert
  safety. Both writes are atomic, so they never expose a half-written backlog to
  a running loop.

## ralphd — Discord control bridge
`ralphd` is a separate, always-on foreground binary that lets one authorized
Discord user drive the loop from one channel via native slash commands. It shells
out to `ralph` and reads/writes `.ralph/`; it is single-tenant by design (one
guild, one channel, one user id — every other channel/user is refused).

```bash
DISCORD_BOT_TOKEN=… ralphd \
  --guild <GUILD_ID> --channel <CHANNEL_ID> --user <USER_ID> \
  [--working-dir <repo>] -- <ralph args forwarded to /start>
```

Commands: `/start`, `/stop`, `/model <tier>`, `/status`, `/next`,
`/backlog-add`, `/backlog-edit`. Loop lifecycle/progress still posts via
`ralph`'s existing `DISCORD_WEBHOOK` (point it at the same channel); ralphd only
handles inbound commands and their replies. Invite the bot with the `bot` +
`applications.commands` scopes.
```

- [ ] **Step 4: Commit**

```bash
git add personalize/scripts/setup_ralph.sh tools/ralph/README.md
git commit -m "ralphd: build+install via setup_ralph.sh; document subcommands and bridge"
```

---

## Done

All nine tasks complete: `ralph` gained `status`/`backlog` subcommands with a lint-or-revert safety gate; `ralphd` is a tested single-tenant Discord bridge; both build and install together. The gateway glue is manually verified; every pure decision (auth, tier validation, pid liveness, backlog mutation, status formatting) is unit-tested.
