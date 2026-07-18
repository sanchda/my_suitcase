# ralph: canonical `.ralph/` layout, `ralph init`, and backlog archiving

Date: 2026-07-17
Status: Approved (design)

## Summary

Three related changes to the `ralph` runner (`tools/ralph/`):

1. **Canonical `.ralph/` directory.** Move ralph's per-repo driving files out of
   `tools/ralph/` and into `.ralph/`, which becomes the single "ralph home" for a
   repo. The binary reads `.ralph/PROMPT.md` and `.ralph/ralph.toml` by default;
   runtime state continues to live directly under `.ralph/`. A whitelist
   `.gitignore` block commits the config files and ignores generated state.
2. **`ralph init` subcommand.** Scaffolds `.ralph/` in the current repo:
   the config files (from an embedded copy of the prompt template), starter
   stubs, an `archive/` directory, and the `.gitignore` block. Idempotent.
3. **Backlog archiving on completion.** When the loop finishes via the completion
   marker, the runner moves the active `BACKLOG.md` into `.ralph/archive/`
   (timestamped). `git mv` + commit when tracked; plain rename otherwise.

No migration path is provided — the tool has no external users yet, so `.ralph/`
is simply treated as canonical from now on.

## Motivation

Today the runner splits driving files between a committed `tools/ralph/`
directory (PROMPT, ralph.toml, VISION/BACKLOG/PROGRESS) and a gitignored `.ralph/`
runtime directory. Consolidating everything under `.ralph/` gives one obvious
place to look, and `ralph init` removes the manual copy-the-template setup step.
Archiving a finished backlog keeps a record of completed work without leaving it
in the active backlog file.

## Background: what the runner actually reads

The binary only opens two files itself: the prompt (`cfg.prompt`) and the config
file (`cfg.config`). `VISION`/`BACKLOG`/`PROGRESS` are conventions the *agent*
follows because `PROMPT.template.md` tells it to — the runner never parsed them
before this change. Backlog archiving therefore requires teaching the runner
where the backlog file is (new `backlog` config) so it can move it on completion.

Completion is detected in `control.rs` when the marker appears on its own line in
the result text (around line 199, the `Class::Success` branch). That is the hook
point for archiving.

Runtime state written under `.ralph/` today (see `state.rs`): `iteration`,
`MODEL`, `STATUS`, `STOP`, `live`, `run.log`, `current.log`, `logs/`,
`last-result.json`, `git-baseline`.

## Design

### 1. Directory layout (`.ralph/` as canonical home)

`.ralph/` holds both committed config and generated runtime state; the split is
by filename (Approach A — runtime files stay at the top level so the documented
operator paths like `.ralph/STOP`, `.ralph/live`, `.ralph/MODEL` are unchanged).

Committed (config / driving files):
- `.ralph/PROMPT.md` — per-iteration prompt (was `tools/ralph/PROMPT.md`)
- `.ralph/ralph.toml` — optional config (was `tools/ralph/ralph.toml`)
- `.ralph/VISION.md`, `.ralph/BACKLOG.md`, `.ralph/PROGRESS.md` — conventions
- `.ralph/archive/` — completed backlogs (with a `.gitkeep`)

Gitignored (generated runtime): everything else under `.ralph/` — `iteration`,
`MODEL`, `STATUS`, `STOP`, `live`, `run.log`, `current.log`, `logs/`,
`last-result.json`, `git-baseline`.

Repo `.gitignore` block (the first line is a sentinel `ralph init` uses to detect
an already-present block — see section 3):

```
# ralph loop home (managed by `ralph init`)
/.ralph/*
!/.ralph/PROMPT.md
!/.ralph/ralph.toml
!/.ralph/VISION.md
!/.ralph/BACKLOG.md
!/.ralph/PROGRESS.md
!/.ralph/archive/
```

`/.ralph/*` ignores every direct child (so git never descends into `logs/`);
the `!` lines re-include the config files and the archive directory.

### 2. Config changes (`config.rs`, `main.rs`)

Default path changes:

| Setting      | Old default             | New default        |
|--------------|-------------------------|--------------------|
| `prompt`     | `tools/ralph/PROMPT.md` | `.ralph/PROMPT.md` |
| config file  | `tools/ralph/ralph.toml`| `.ralph/ralph.toml`|
| `dir`        | `.ralph`                | `.ralph` (unchanged; meaning shifts from "runtime dir" to "ralph home") |

New `backlog` setting, following the existing path-config pattern exactly:
- `Config.backlog: PathBuf`, default `.ralph/BACKLOG.md`
- `FileConfig.backlog: Option<String>` (toml key `backlog`)
- env `RALPH_BACKLOG`
- flag `--backlog <file>`
- precedence: defaults ← file ← env ← flags (same as `prompt`/`dir`)

Archive directory is derived, not a separate setting: `<cfg.dir>/archive`.

`main.rs` USAGE text updated for the new defaults and the `--backlog` flag.

### 3. `ralph init` subcommand (`init.rs`, dispatch in `main.rs`)

`main.rs` gains first-positional-arg dispatch: if the first argv element is
`init`, run `init::run()` and exit; otherwise run the loop as today. `--help`
lists the `init` subcommand.

`ralph init` (run from the repo root) is idempotent — it never overwrites an
existing file, and reports each path as created or skipped:

1. Create `.ralph/` and `.ralph/archive/`, plus `.ralph/archive/.gitkeep` (so the
   otherwise-empty, unignored archive dir is tracked).
2. Write `.ralph/PROMPT.md` from the template embedded in the binary via
   `include_str!("../PROMPT.template.md")`. Embedding keeps the template as the
   single source of truth and makes it available at runtime (the installed binary
   lives in `~/.local/bin`, away from the crate source).
3. Write starter stubs if absent:
   - `.ralph/ralph.toml` — a commented example (all keys commented out).
   - `.ralph/BACKLOG.md` — a short "ordered work list" stub.
   - `.ralph/VISION.md` — a short north-star stub.
   - `.ralph/PROGRESS.md` — seeded with a `Goal:` and a `Next:` line.
4. Ensure the `.gitignore` whitelist block (section 1) is present in the repo's
   root `.gitignore`: append it if the block's marker line is not already found;
   never duplicate it. Create `.gitignore` if it does not exist.
5. Print next steps (fill in `.ralph/PROMPT.md`, then run `ralph`).

Idempotency detail: the `.gitignore` block is detected by searching for a
sentinel comment line written above the block, e.g.
`# ralph loop home (managed by `ralph init`)`. If that line is present, the block
is left untouched.

### 4. Backlog archiving on completion (`control.rs`, `git.rs`, `state.rs`)

At the completion point in the `Class::Success` branch (where the marker is seen
and the loop is about to `break` with the COMPLETE log line), call an archive
helper before breaking:

- If `cfg.backlog` does not exist: do nothing (log nothing, or a debug line).
- Otherwise compute the destination `<cfg.dir>/archive/BACKLOG-<timestamp>.md`,
  where `<timestamp>` is the existing UTC `timestamp()` from `state.rs` (format
  `YYYYMMDDTHHMMSSZ`). Ensure the archive dir exists.
- Move the file:
  - If the repo is a git work tree AND the backlog file is tracked, run
    `git mv <backlog> <dest>` then `git commit -m "chore(ralph): archive
    completed backlog"` committing only that move. This leaves the working tree
    clean at end-of-run and records the archived backlog.
  - Otherwise (not a repo, or backlog untracked), do a plain filesystem rename
    (`fs::rename`), falling back to copy+remove if rename crosses devices.
- The whole operation is best-effort: any failure is logged as a warning and the
  run still reports COMPLETE. Archiving must never turn a successful run into a
  failure.

This introduces the runner's first git *write* (previously it only read git
state). This is a deliberate, narrow exception justified by leaving a clean tree.

New helpers:
- `git.rs`: `is_tracked(dir, path) -> bool` (`git ls-files --error-unmatch`),
  `mv_and_commit(dir, from, to, msg) -> bool` (best-effort; returns success).
- Archive logic lives in `control.rs` (or a small `archive` fn there), using
  `git.rs` helpers and `state.rs::timestamp()` (make `timestamp()` `pub(crate)`).

## Files touched

- `tools/ralph/src/config.rs` — new `backlog` field across `Config`/`FileConfig`/
  `apply_file`/`apply_env`/`apply_args`; default path changes; `config_path`
  default → `.ralph/ralph.toml`.
- `tools/ralph/src/main.rs` — subcommand dispatch for `init`; USAGE updates.
- `tools/ralph/src/init.rs` — new module implementing `ralph init`.
- `tools/ralph/src/control.rs` — call the archive helper at completion.
- `tools/ralph/src/git.rs` — `is_tracked`, `mv_and_commit` helpers.
- `tools/ralph/src/state.rs` — expose `timestamp()` to the crate.
- `tools/ralph/PROMPT.template.md` — path hints reference `.ralph/…`.
- `tools/ralph/README.md` — layout table, gitignore step, `ralph init` docs,
  config table (`backlog` row, new defaults).
- `personalize/scripts/setup_ralph.sh` — header comment references `.ralph/`.

## Testing

- `config.rs`: update `defaults_are_sane` / `config_path_precedence` for the new
  default paths; add a `backlog` precedence test (file < env < flag).
- `git.rs`: test `is_tracked` (tracked vs untracked file in a temp repo) and
  `mv_and_commit` (moves file, advances HEAD, tree clean afterward).
- Archive behavior: a test that, in a temp repo with a tracked `.ralph/BACKLOG.md`,
  invokes the archive helper and asserts the file now lives under `archive/` with
  a timestamped name and the tracked tree is clean. A second case with an
  untracked backlog asserts a plain rename occurred.
- `init.rs`: a test in a temp dir asserting the created files/dirs exist, a second
  `init` run skips existing files, and the `.gitignore` block is present exactly
  once after two runs.
- Full `cargo test` and `cargo build --release` green.

## Non-goals

- No migration from `tools/ralph/` to `.ralph/` (no existing users).
- No support for multiple sequential backlogs in one run (archive fires once, on
  completion). Multi-backlog workflows can be a later change.
- Runtime files are not relocated into a `.ralph/run/` subdirectory (Approach B
  was rejected to preserve documented operator paths).
