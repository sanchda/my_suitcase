# ralph `.ralph/` layout, `init`, and backlog archiving — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `.ralph/` the canonical per-repo home for ralph (prompt, config, backlog), add a `ralph init` scaffolding subcommand, and archive the backlog into `.ralph/archive/` when a run completes.

**Architecture:** Small, additive changes to an existing Rust CLI crate (`tools/ralph/`). Config gains a `backlog` path and new default paths; `main.rs` gains subcommand dispatch to a new `init` module; the control loop calls a best-effort archive helper at the completion marker; git gains two read/write helpers. Docs follow the code.

**Tech Stack:** Rust (std only + `serde`/`toml` already in the crate), `git` CLI shelled out via `std::process::Command`.

**Working directory for all commands:** `/home/sanchda/suitcase/tools/ralph` (the cargo crate). Run tests with `cargo test`; build with `cargo build --release`.

---

## File Structure

- `src/config.rs` — add `backlog` field + precedence wiring; change `prompt` and config-file default paths.
- `src/main.rs` — subcommand dispatch (`init`); USAGE text updates; `mod init;`.
- `src/init.rs` — **new** — `ralph init` scaffolding, idempotent, unit-testable via `run_in(root)`.
- `src/git.rs` — add `is_tracked` and `mv_and_commit` helpers + tests.
- `src/state.rs` — make `timestamp()` crate-visible.
- `src/control.rs` — call `archive_backlog(...)` at the completion marker; add the helper + a test.
- `PROMPT.template.md` — path hints reference `.ralph/…`.
- `README.md` — layout table, gitignore step, `ralph init` docs, config table.
- `../../personalize/scripts/setup_ralph.sh` — header comment references `.ralph/`.

---

## Task 1: Add the `backlog` config setting

**Files:**
- Modify: `src/config.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/config.rs`:

```rust
    #[test]
    fn backlog_precedence() {
        let mut c = Config::default();
        assert_eq!(c.backlog, PathBuf::from(".ralph/BACKLOG.md"));
        apply_file(&mut c, toml::from_str(r#"backlog = "a/B.md""#).unwrap()).unwrap();
        assert_eq!(c.backlog, PathBuf::from("a/B.md"));
        apply_env(&mut c, |k| (k == "RALPH_BACKLOG").then(|| "b/B.md".to_string())).unwrap();
        assert_eq!(c.backlog, PathBuf::from("b/B.md"));
        apply_args(&mut c, &["--backlog".into(), "c/B.md".into()]).unwrap();
        assert_eq!(c.backlog, PathBuf::from("c/B.md"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib backlog_precedence`
Expected: FAIL — compile error, `Config` has no field `backlog` (and `backlog` toml key rejected by `deny_unknown_fields`).

- [ ] **Step 3: Add the field and wiring**

In `struct Config` (after `pub dir: PathBuf,`) add:

```rust
    /// Backlog file to archive on completion.
    pub backlog: PathBuf,
```

In `impl Default for Config` (after `dir: PathBuf::from(".ralph"),`) add:

```rust
            backlog: PathBuf::from(".ralph/BACKLOG.md"),
```

In `struct FileConfig` (after `pub dir: Option<String>,`) add:

```rust
    pub backlog: Option<String>,
```

In `apply_file` (after the `if let Some(v) = f.dir { ... }` block) add:

```rust
    if let Some(v) = f.backlog {
        cfg.backlog = PathBuf::from(v);
    }
```

In `apply_env` (after the `if let Some(v) = get("RALPH_DIR") { ... }` block) add:

```rust
    if let Some(v) = get("RALPH_BACKLOG") {
        cfg.backlog = PathBuf::from(v);
    }
```

In `apply_args`, in the `match a.as_str()` arm list (after the `"--dir" =>` arm) add:

```rust
            "--backlog" => cfg.backlog = PathBuf::from(next()?),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib backlog_precedence`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(ralph): add backlog config setting (file/env/flag)"
```

---

## Task 2: Move default paths to `.ralph/`

**Files:**
- Modify: `src/config.rs`, `src/main.rs`

- [ ] **Step 1: Update the failing tests first**

In `src/config.rs`, in `fn defaults_are_sane`, add after the existing assertions:

```rust
        assert_eq!(c.prompt, PathBuf::from(".ralph/PROMPT.md"));
```

In `fn config_path_precedence`, change the last assertion from `tools/ralph/ralph.toml` to:

```rust
        assert_eq!(config_path(&[], |_| None), PathBuf::from(".ralph/ralph.toml"));
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib defaults_are_sane config_path_precedence`
Expected: FAIL — `prompt` is still `tools/ralph/PROMPT.md` and `config_path` default is still `tools/ralph/ralph.toml`.

- [ ] **Step 3: Change the defaults**

In `impl Default for Config`, change the `prompt` line to:

```rust
            prompt: PathBuf::from(".ralph/PROMPT.md"),
```

In `fn config_path`, change the final return to:

```rust
    PathBuf::from(".ralph/ralph.toml")
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib`
Expected: PASS (all config tests green)

- [ ] **Step 5: Update USAGE text in `src/main.rs`**

In the `USAGE` const, replace the three affected lines so they read:

```rust
  --prompt <file>          Prompt fed each iteration (default .ralph/PROMPT.md)
```
```rust
  --backlog <file>         Backlog archived on completion (default .ralph/BACKLOG.md)
```
```rust
  --config <file>          Config file (default .ralph/ralph.toml)
```

Add the `--backlog` line immediately after the `--prompt` line (the other two replace existing lines in place).

- [ ] **Step 6: Verify it builds**

Run: `cargo build`
Expected: builds clean, no warnings about the new text.

- [ ] **Step 7: Commit**

```bash
git add src/config.rs src/main.rs
git commit -m "feat(ralph): default prompt/config/backlog paths to .ralph/"
```

---

## Task 3: git helpers — `is_tracked` and `mv_and_commit`

**Files:**
- Modify: `src/git.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/git.rs`:

```rust
    #[test]
    fn track_check_and_move_commit() {
        let dir = temp_repo();
        fs::create_dir_all(dir.join("archive")).unwrap();
        fs::write(dir.join("BACKLOG.md"), "work").unwrap();
        // Untracked yet.
        assert!(!is_tracked(&dir, &dir.join("BACKLOG.md")));
        run(&dir, &["add", "BACKLOG.md"]);
        run(&dir, &["commit", "-qm", "add backlog"]);
        assert!(is_tracked(&dir, &dir.join("BACKLOG.md")));

        let before = head(&dir);
        let dest = dir.join("archive/BACKLOG-x.md");
        assert!(mv_and_commit(&dir, &dir.join("BACKLOG.md"), &dest, "archive"));
        // File moved, HEAD advanced, tracked tree clean.
        assert!(!dir.join("BACKLOG.md").exists());
        assert!(dest.exists());
        assert!(advanced_since(&dir, &before));
        assert!(tracked_dirt(&dir).is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib track_check_and_move_commit`
Expected: FAIL — `is_tracked` / `mv_and_commit` not defined.

- [ ] **Step 3: Implement the helpers**

Add to `src/git.rs` (after `pub fn newly_dirty(...)`, before the `#[cfg(test)]` module):

```rust
/// Is `path` tracked by git in the work tree at `dir`?
pub fn is_tracked(dir: &Path, path: &Path) -> bool {
    git(dir, &["ls-files", "--error-unmatch", &path.to_string_lossy()]).is_some()
}

/// `git mv from to` then commit only that move with `msg`. Best-effort: returns
/// whether both steps succeeded; leaves the tree as git left it on failure.
pub fn mv_and_commit(dir: &Path, from: &Path, to: &Path, msg: &str) -> bool {
    let from = from.to_string_lossy();
    let to = to.to_string_lossy();
    if git(dir, &["mv", &from, &to]).is_none() {
        return false;
    }
    git(dir, &["commit", "-m", msg, "--", &to, &from]).is_some()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib track_check_and_move_commit`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/git.rs
git commit -m "feat(ralph): add git is_tracked and mv_and_commit helpers"
```

---

## Task 4: Archive the backlog on completion

**Files:**
- Modify: `src/state.rs` (expose `timestamp`), `src/control.rs` (helper + call + test)

- [ ] **Step 1: Expose `timestamp()` to the crate**

In `src/state.rs`, change the signature of the private timestamp helper from:

```rust
fn timestamp() -> String {
```
to:
```rust
pub(crate) fn timestamp() -> String {
```

- [ ] **Step 2: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/control.rs` (it already imports `super::*`; add any extra `use` lines shown):

```rust
    #[test]
    fn archive_moves_tracked_backlog() {
        use std::fs;
        use std::path::PathBuf;
        use std::process::Command;
        let repo = std::env::temp_dir().join(format!("ralph-arch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&repo);
        fs::create_dir_all(repo.join(".ralph")).unwrap();
        for a in [["init", "-q"], ["config", "user.email", "t@t"], ["config", "user.name", "t"]] {
            Command::new("git").arg("-C").arg(&repo).args(a).output().unwrap();
        }
        fs::write(repo.join(".ralph/BACKLOG.md"), "items").unwrap();
        Command::new("git").arg("-C").arg(&repo).args(["add", ".ralph/BACKLOG.md"]).output().unwrap();
        Command::new("git").arg("-C").arg(&repo).args(["commit", "-qm", "seed"]).output().unwrap();

        let cfg = Config {
            dir: repo.join(".ralph"),
            backlog: repo.join(".ralph/BACKLOG.md"),
            ..Config::default()
        };
        let state = State::open(&cfg.dir).unwrap();
        archive_backlog(&cfg, &state, &repo);

        assert!(!cfg.backlog.exists(), "backlog should be moved");
        let archive = repo.join(".ralph/archive");
        let moved: Vec<PathBuf> = fs::read_dir(&archive).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(moved.len(), 1);
        assert!(moved[0].file_name().unwrap().to_string_lossy().starts_with("BACKLOG-"));
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --lib archive_moves_tracked_backlog`
Expected: FAIL — `archive_backlog` not defined.

- [ ] **Step 4: Implement the helper**

In `src/control.rs`, add this function near the other free functions (e.g. just above `pub fn run`):

```rust
/// On completion, move the backlog file into `<dir>/archive/` (timestamped).
/// Best-effort: any failure is logged and ignored so a finished run stays done.
fn archive_backlog(cfg: &Config, state: &State, repo: &Path) {
    if !cfg.backlog.exists() {
        return;
    }
    let archive_dir = cfg.dir.join("archive");
    if let Err(e) = std::fs::create_dir_all(&archive_dir) {
        state.log(&format!("  ⚠ could not create archive dir: {e}"));
        return;
    }
    let dest = archive_dir.join(format!("BACKLOG-{}.md", crate::state::timestamp()));
    let moved = if git::is_repo(repo) && git::is_tracked(repo, &cfg.backlog) {
        git::mv_and_commit(repo, &cfg.backlog, &dest, "chore(ralph): archive completed backlog")
    } else {
        std::fs::rename(&cfg.backlog, &dest).is_ok()
    };
    if moved {
        state.log(&format!("  archived backlog → {}", dest.display()));
    } else {
        state.log(&format!("  ⚠ could not archive backlog {}", cfg.backlog.display()));
    }
}
```

- [ ] **Step 5: Call it at the completion marker**

In `pub fn run`, in the `Class::Success` branch, change the marker block from:

```rust
                if stream::has_marker(&text, &cfg.marker) {
                    state.log("  marker seen (own line) → COMPLETE");
                    state.log(&format!("=== ralph COMPLETE after {iter} iterations ==="));
                    break;
                }
```
to:
```rust
                if stream::has_marker(&text, &cfg.marker) {
                    state.log("  marker seen (own line) → COMPLETE");
                    archive_backlog(cfg, &state, repo);
                    state.log(&format!("=== ralph COMPLETE after {iter} iterations ==="));
                    break;
                }
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test --lib archive_moves_tracked_backlog`
Expected: PASS

- [ ] **Step 7: Run the full suite**

Run: `cargo test`
Expected: PASS (all tests green)

- [ ] **Step 8: Commit**

```bash
git add src/state.rs src/control.rs
git commit -m "feat(ralph): archive backlog into .ralph/archive on completion"
```

---

## Task 5: `ralph init` subcommand

**Files:**
- Create: `src/init.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Create `src/init.rs` with the scaffolding logic**

Create `src/init.rs`:

```rust
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
# extra_args = [\"--add-dir\", \"/some/path\"]
";

const BACKLOG_STUB: &str = "\
# Backlog

Ordered work list — the loop takes the first unfinished item unless PROGRESS's
\"Next:\" says otherwise. Check items off as they land.

- [ ] First item
";

const VISION_STUB: &str = "\
# Vision

The north star for this loop — what \"done\" looks like and why. Keep it short.
";

const PROGRESS_STUB: &str = "\
# Progress

Goal: {{fill in the one- or two-sentence goal}}

Next: {{the first concrete step}}

## Log
";

/// Result of scaffolding: which paths were created vs already present.
pub struct Report {
    pub created: Vec<String>,
    pub skipped: Vec<String>,
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
    println!("\n.ralph/ ready. Fill in .ralph/PROMPT.md, then run `ralph`.");
    Ok(0)
}

/// Scaffold `.ralph/` under `root`. Idempotent — never overwrites.
pub fn run_in(root: &Path) -> R<Report> {
    let mut report = Report { created: Vec::new(), skipped: Vec::new() };
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

fn ensure_gitignore(path: &Path, report: &mut Report) -> R<()> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    if existing.contains(GITIGNORE_SENTINEL) {
        report.skipped.push(".gitignore (ralph block present)".to_string());
        return Ok(());
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

    fn tmp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ralph-init-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn scaffolds_then_is_idempotent() {
        let root = tmp();
        let r1 = run_in(&root).unwrap();
        // Everything created on the first run (6 files + gitignore block).
        assert!(r1.created.iter().any(|p| p == ".ralph/PROMPT.md"));
        assert!(root.join(".ralph/PROMPT.md").exists());
        assert!(root.join(".ralph/archive/.gitkeep").exists());
        assert!(root.join(".gitignore").exists());

        // Second run skips existing files and does not duplicate the block.
        let r2 = run_in(&root).unwrap();
        assert!(r2.created.is_empty(), "second init should create nothing: {:?}", r2.created);
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
}
```

- [ ] **Step 2: Wire the module and dispatch in `src/main.rs`**

Add to the module list in `src/main.rs` (with the other `mod` lines):

```rust
mod init;
```

At the top of `fn run()`, immediately after `let argv: Vec<String> = std::env::args().skip(1).collect();`, add:

```rust
    if argv.first().map(String::as_str) == Some("init") {
        return init::run();
    }
```

- [ ] **Step 3: Add `init` to the USAGE text in `src/main.rs`**

In the `USAGE` const, change the usage line and add a subcommand section. Replace:

```rust
Usage: ralph [options]
```
with:
```rust
Usage: ralph [options]
       ralph init                Scaffold .ralph/ in the current repo
```

- [ ] **Step 4: Run the init tests**

Run: `cargo test --lib init`
Expected: PASS (`scaffolds_then_is_idempotent`, `appends_gitignore_block_to_existing`)

- [ ] **Step 5: Manually verify the subcommand end-to-end**

```bash
cargo build --release
cd "$(mktemp -d)" && git init -q && "$OLDPWD/target/release/ralph" init && ls -a .ralph && cat .gitignore
cd "$OLDPWD"
```
Expected: `.ralph/` contains `PROMPT.md ralph.toml BACKLOG.md VISION.md PROGRESS.md archive/`; `.gitignore` ends with the ralph block. (Clean up the temp dir afterward.)

- [ ] **Step 6: Commit**

```bash
git add src/init.rs src/main.rs
git commit -m "feat(ralph): add 'ralph init' scaffolding subcommand"
```

---

## Task 6: Documentation

**Files:**
- Modify: `PROMPT.template.md`, `README.md`, `../../personalize/scripts/setup_ralph.sh`

- [ ] **Step 1: Update `PROMPT.template.md` path hints**

- Line ~5–7 comment: change "copy this to your repo (default path tools/ralph/PROMPT.md)" to "copy this to `.ralph/PROMPT.md` (or run `ralph init`)".
- Line ~11: change the PROGRESS example path from `tools/ralph/PROGRESS.md` to `.ralph/PROGRESS.md`.
- Any other `tools/ralph/` occurrences in this file → `.ralph/`.

Verify none remain:
Run: `grep -n "tools/ralph" PROMPT.template.md`
Expected: no output.

- [ ] **Step 2: Update `README.md`**

Make these edits:
- The "Global tool vs. local driving files" table: change each driving-file path from `tools/ralph/…` to `.ralph/…` (PROMPT.md, VISION.md, BACKLOG.md, PROGRESS.md, ralph.toml). Keep the runtime row as `.ralph/` generated state.
- Add a sentence under that section: config files under `.ralph/` are committed; generated runtime state under `.ralph/` is gitignored (see the gitignore block below).
- "Quick start" section: replace step 1 (`cp "$SUITCASE/tools/ralph/PROMPT.template.md" tools/ralph/PROMPT.md …`) with `ralph init` and "fill in every `{{...}}` in `.ralph/PROMPT.md`". Replace step 2's `tools/ralph/…` paths with `.ralph/…`. Replace step 3 (`Add .ralph/ to .gitignore`) with a note that `ralph init` writes the gitignore block.
- Add the gitignore block to the README (fenced), matching `GITIGNORE_BLOCK` in `src/init.rs` including the sentinel comment line.
- Config table: change the `prompt` default to `.ralph/PROMPT.md`, the `RALPH_CONFIG`/`--config` default to `.ralph/ralph.toml`; add a `backlog | RALPH_BACKLOG | --backlog | .ralph/BACKLOG.md` row.
- Add a short "Completion → archive" note: on completion the runner moves the backlog to `.ralph/archive/BACKLOG-<timestamp>.md` (git mv + commit when tracked, plain move otherwise).
- Development/Modules line: add `init` to the module list.

Verify no stale driving-file paths remain (the crate's own source path `tools/ralph/` references in prose about the crate location may stay; driving-file paths must not):
Run: `grep -n "tools/ralph/PROMPT\|tools/ralph/BACKLOG\|tools/ralph/VISION\|tools/ralph/PROGRESS\|tools/ralph/ralph.toml" README.md`
Expected: no output.

- [ ] **Step 3: Update `../../personalize/scripts/setup_ralph.sh` header**

Change the comment line that reads `(tools/ralph/PROMPT.md, VISION/BACKLOG/PROGRESS, and an optional ralph.toml).` to reference `.ralph/` (e.g. "(.ralph/PROMPT.md, VISION/BACKLOG/PROGRESS, and an optional ralph.toml — run `ralph init` to scaffold).").

- [ ] **Step 4: Full build + test sanity**

Run: `cargo build --release && cargo test`
Expected: build clean, all tests PASS.

- [ ] **Step 5: Commit**

```bash
git add README.md PROMPT.template.md ../../personalize/scripts/setup_ralph.sh
git commit -m "docs(ralph): document .ralph/ layout, ralph init, and backlog archiving"
```

---

## Final verification

- [ ] Run `cargo test` — all green.
- [ ] Run `cargo build --release` — clean.
- [ ] `grep -rn "tools/ralph/PROMPT\|tools/ralph/BACKLOG\|tools/ralph/VISION\|tools/ralph/PROGRESS\|tools/ralph/ralph.toml" src README.md PROMPT.template.md` returns nothing (no stale driving-file defaults).
- [ ] Manual smoke test from Task 5 Step 5 still produces a correct `.ralph/` and gitignore.

## Notes for the implementer

- The runner shells out to `git` and accepts absolute paths inside `git -C <repo>` (git normalizes them to the work tree). If `git mv` ever rejects an absolute pathspec in practice, fall back to passing paths relative to `repo`.
- `archive_backlog` is intentionally best-effort: it must never convert a COMPLETE run into a failure. Keep every error path as a `state.log` warning + return.
- Do not add a migration path from `tools/ralph/` — `.ralph/` is canonical and the tool has no external users yet.
```
