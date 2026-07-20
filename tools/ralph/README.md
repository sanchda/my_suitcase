# ralph — external autonomous loop

Runs `claude -p` in a loop, **fresh context each iteration**, feeding a stable
base prompt plus a bounded current-task brief until a completion marker appears.
This is the "pure Ralph" (Geoffrey Huntley) external form — each call starts
cold and stays cheap, so it suits context-expensive / thinking models.
Cross-iteration state lives in files, not context.

This is the Rust runner (`tools/ralph/`, a cargo crate). Beyond looping it adds
**live stream parsing**, **cost / wall-clock budgets**, an **opt-in
per-iteration timeout**, **schema-validated backlog routing**, bounded iteration
briefs, and **no-progress detection** that escalates the model tier and then
aborts. It replaces the previous `ralph.sh`.

## Global tool vs. local driving files

This directory (`$SUITCASE/tools/ralph/`) is the **global, project-agnostic
tool**. Its personalize script builds the binary and installs it to
`~/.local/bin/ralph`, so `ralph` is on your PATH. The crate contains nothing
about any one project.

Everything that *drives* a run is **local to the repo you run it in**:

| Kind | File (default path) | Global or local? |
|------|---------------------|------------------|
| Runner | `ralph` (this tool) | **global** — on PATH |
| Per-iteration prompt | `.ralph/PROMPT.md` | **local** (copy `PROMPT.template.md`, or run `ralph init`) |
| North star (optional) | `.ralph/VISION.md` | **local** |
| Ordered backlog (optional) | `.ralph/BACKLOG.md` | **local** |
| Durable memory / log | `.ralph/PROGRESS.md` | **local** |
| Config (optional) | `.ralph/ralph.toml` | **local**, committed |
| Runtime (counter, logs, MODEL/STATUS) | `.ralph/` (gitignored) | **local**, generated |

Rule of thumb: **the runner is global; the prompts, config, and record-keeping
are local.**

`.ralph/` holds both kinds of file: the driving files above (PROMPT.md,
VISION.md, BACKLOG.md, PROGRESS.md, ralph.toml) are **committed**, while the
generated runtime state (counters, logs, MODEL/STATUS, etc.) is **gitignored**
— see the gitignore block below, which `ralph init` writes for you.

## Install

Build and install via the suitcase personalize script (requires the Rust
toolchain; `claude` must be authenticated on PATH at runtime):

```bash
$SUITCASE/personalize/scripts/setup_ralph.sh
# or, with everything else: $SUITCASE/personalize/personalize
```

Rebuild after source changes by re-running that script.

## Quick start (in the repo you want worked on)

1. Run `ralph init` to scaffold `.ralph/` (PROMPT.md, ralph.toml, BACKLOG.md,
   VISION.md, PROGRESS.md, an `archive/` dir, and the `.gitignore` block
   below). Then fill in every `{{...}}` in `.ralph/PROMPT.md` — the GOAL, the
   verification command, the commit contract.
2. Flesh out `.ralph/BACKLOG.md` using the v1 schema, optionally add a VISION,
   and seed PROGRESS with the goal + `Next: <task-id> — <step>`.
3. `ralph init` already wrote the `.gitignore` block for you (see below) — no
   manual step needed.
4. Check routing, then run it on a dedicated branch:

   ```bash
   ralph lint
   ralph brief
   ralph --max-iterations 30      # from the repo root
   ```

   Test a single pass first with `ralph --once`.

Run **one `ralph` per worktree** — each loop drives the repo it is launched in.

### `ralph init`
Scaffolds `.ralph/` in the current repo: writes `PROMPT.md` (from the
template), stub `ralph.toml` / `BACKLOG.md` / `VISION.md` / `PROGRESS.md`
files, an `archive/` directory, and appends the ralph `.gitignore` block
(below) to the repo's `.gitignore`. Idempotent — it never overwrites a file
that already exists, and running it again just reports what's already there.

The `.gitignore` block `ralph init` writes (idempotent — it won't duplicate
this if it's already present):

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

This whitelists the committed driving files while leaving everything else
under `.ralph/` (runtime state) gitignored.

## Deterministic backlog schema and staging

The v1 schema is plain Markdown. A task is a checkbox with a unique ID, a bold
`ID — title` label, and a verification contract:

```markdown
<!-- ralph-backlog: v1 -->
# Backlog

- [ ] **36.8 — Ship weighted selection end-to-end.**
  Explain the outcome and constraints.
  Verify: cargo test -p generator
```

Large work can be staged with two-space-indented child tasks whose IDs extend
the parent:

```markdown
- [ ] **36.8 — Ship weighted selection end-to-end.**
  Verify: cargo test && ./tools/verify_runtime.sh
  - [x] **36.8.1 — Emit the schema.**
    Verify: cargo test -p generator schema
  - [ ] **36.8.2 — Consume it at runtime.**
    Verify: ./tools/verify_runtime.sh weighted_selection
```

Ralph selects the first unchecked task with no unchecked descendants. A parent
with pending children is a container; after all children are checked, the
parent becomes the final integration/closure step. `Next:` may refine the
selected leaf but cannot override backlog order.

If a selected leaf is too large for one pass, the agent first adds named child
stages and runs the linter. Routing state belongs in BACKLOG, not in a growing
sequence of informal “slice N” hand-offs in PROGRESS.

`ralph lint` validates IDs, nesting, status consistency, and `Verify:` on every
pending task. `ralph brief` prints the exact bounded task context that would be
appended to the next Claude prompt. The loop reruns the same validation at every
iteration boundary and refuses to guess when schema errors exist. See
[BACKLOG.schema.md](BACKLOG.schema.md) for the complete contract.

## Watching / controlling a running loop
- **Live status of the active iteration** (tool, elapsed, output tokens, last
  activity): `cat .ralph/live`
- **Raw stream of the active iteration** (includes thinking): `tail -f .ralph/current.log`
- **High-level progress:** `tail -f .ralph/run.log`
- **Stop gracefully** after the current iteration: `touch .ralph/STOP`
- **Resume** later: just re-run `ralph` — the counter in `.ralph/iteration`
  persists.
- **Launch detached** for overnight runs: `nohup setsid ralph … &`.

Each completed result adds a `perf` line to `run.log` with total, API, and
non-API time, turn count, and token/cache totals. This makes model time versus
local tools/tests visible without mining raw NDJSON.

## Completion
The loop ends when the model's **final text** (from the result envelope's
`.result`, which excludes thinking) contains the marker token on its own line,
default `RALPH_COMPLETE`. Your `PROMPT.md` must instruct the model to emit it
only when the whole goal is genuinely done and verified.

### Completion → archive
On completion, the runner moves the backlog file into
`.ralph/archive/BACKLOG-<timestamp>.md` — `git mv` + a commit when the backlog
is tracked, a plain filesystem rename otherwise. This is best-effort: a
finished run is never turned into a failure by an archive hiccup.

## Per-iteration hand-offs (the agent writes these)
Each iteration ends by writing two one-word files that steer the next step:

- `.ralph/MODEL` — `haiku` / `sonnet` / `opus`, sizing the NEXT iteration
  (mechanical → haiku, normal → sonnet, hard/repeatedly-failing → opus).
- `.ralph/STATUS` — this iteration's type: `code` (a normal committing
  iteration), or `review`/`plan`/`blocked` for an intentional non-code pass.
  Absent is treated as `code`.

Invalid `MODEL` values are ignored with a warning (never abort). See the PROMPT
template for the exact instructions given to the model.

Before every process launch the runner parses the complete backlog and appends
a bounded brief containing the selected task/stage plus the first `Next:` only
when it names that same task. Later historical hand-offs are never searched as
fallback routing. The base prompt remains first and stable for caching. Ralph
also passes `--no-session-persistence` (iterations are deliberately fresh) and
`--exclude-dynamic-system-prompt-sections` (better prompt-cache reuse).

## No-progress detection & escalation
A **progress streak** counts consecutive unproductive iterations. An iteration
is **no-progress** when it is a `code` iteration that made no new commit, or it
was a transient/timeout retry. A declared non-`code` pass (`review`/`blocked`/…)
is **excluded** and logged as such. On the streak reaching:

- `--escalate-after` (default 2): the model escalates one tier up the ladder
  `haiku → sonnet → opus` for the next attempt;
- `--abort-after` (default 4): the loop aborts with a clear reason.

A productive `code` iteration resets the streak.

## Budgets
Checked at iteration boundaries; each halts the loop when hit:

| Budget | Flag / env | Default |
|--------|-----------|---------|
| Cumulative cost | `--max-cost` / `RALPH_MAX_COST` | 0 (off) |
| Wall-clock | `--max-duration` / `RALPH_MAX_DURATION` (`8h`/`30m`/`300s`) | 0 (off) |
| Iterations | `--max-iterations` / `RALPH_MAX_ITER` | 0 (off) |

## Per-iteration timeout
Off by default. With `--iteration-timeout <dur>` (or `RALPH_ITER_TIMEOUT`), an
iteration running longer than the deadline is killed (its whole process group,
so `claude` and its tool subprocesses go too) and treated as a transient retry;
repeated timeouts feed no-progress detection and eventually abort.

## Robustness against running out of usage credits
The Claude CLI returns **exit 0 even on API errors**, so the runner ignores exit
codes and parses the JSON result envelope (`is_error`, `api_error_status`,
`.result`). Errors are classified:

| Class | Trigger | Behavior |
|-------|---------|----------|
| **LIMIT** | 429, or text matching `usage limit` / `credit balance` / `quota` / `will reset` / `rate limit` | Wait it out. Unlimited retries, capped exponential backoff (`RALPH_LIMIT_WAIT`=300s → `RALPH_LIMIT_WAIT_MAX`=3600s). Never counts as no-progress. |
| **TRANSIENT** | 5xx / `overloaded` / network / timeout / empty output (crash/kill) | Short capped backoff (10s → 300s), retried; counts toward no-progress so a truly stuck iteration eventually escalates/aborts. |
| **FATAL** | 401/403 auth, 400/404 bad model / invalid request | Abort with a clear message — looping won't fix config. |

The full error text is logged to the iteration log and `.ralph/last-result.json`.

## Committing (legible incremental history)
Run on a dedicated branch; the PROMPT tells the agent to **commit once per
verified `code` iteration**, so history reads as one clean step per commit. The
prompt must instruct it to stage only files it changed this iteration by explicit
path (never `git add -A`), commit only when verification passed, and never
`git reset`/rebase/amend/switch branches. The runner logs a `⚠ … newly-dirty`
warning if the tracked tree is still dirty after an iteration.

## Configuration
Precedence: **defaults ← `.ralph/ralph.toml` ← env (`RALPH_*`) ← flags**.
`ralph.toml` is optional; absent → all defaults. `ralph --help` lists every flag.

Example `.ralph/ralph.toml`:

```toml
model = "sonnet"
fallback_model = "sonnet"
effort = "auto"
max_cost_usd = 25.0
max_duration = "8h"
iteration_timeout = "45m"
escalate_after = 2
abort_after = 4
# extra_args = ["--add-dir", "/some/path"]
```

| Key (toml) | Env | Flag | Default |
|---|---|---|---|
| `model` | `RALPH_MODEL` | `--model` | `sonnet` |
| `fallback_model` | `RALPH_FALLBACK_MODEL` | `--fallback-model` | `sonnet` |
| `effort` | `RALPH_EFFORT` | `--effort` | `auto` |
| `max_iterations` | `RALPH_MAX_ITER` | `--max-iterations` | `0` |
| `max_cost_usd` | `RALPH_MAX_COST` | `--max-cost` | `0` |
| `max_duration` | `RALPH_MAX_DURATION` | `--max-duration` | `0` |
| `iteration_timeout` | `RALPH_ITER_TIMEOUT` | `--iteration-timeout` | `0` |
| `escalate_after` | `RALPH_ESCALATE_AFTER` | `--escalate-after` | `2` |
| `abort_after` | `RALPH_ABORT_AFTER` | `--abort-after` | `4` |
| `marker` | `RALPH_MARKER` | `--marker` | `RALPH_COMPLETE` |
| `prompt` | `RALPH_PROMPT` | `--prompt` | `.ralph/PROMPT.md` |
| `backlog` | `RALPH_BACKLOG` | `--backlog` | `.ralph/BACKLOG.md` |
| `progress` | `RALPH_PROGRESS` | `--progress` | `.ralph/PROGRESS.md` |
| `dir` | `RALPH_DIR` | `--dir` | `.ralph` |
| `yolo` | `RALPH_YOLO` | `--no-yolo` | `true` |
| `limit_wait` / `_max` | `RALPH_LIMIT_WAIT[_MAX]` | — | 300 / 3600 |
| `transient_wait` / `_max` | `RALPH_TRANSIENT_WAIT[_MAX]` | — | 10 / 300 |
| `extra_args` | `RALPH_EXTRA_ARGS` | — | — |
| `escalation_ladder` | — | — | `["haiku","sonnet","opus"]` |
| — | `RALPH_CONFIG` | `--config` | `.ralph/ralph.toml` |
| — | — | `--once` | run one iteration then exit |

`--dangerously-skip-permissions` is on by default (`--no-yolo` disables) — an
unattended loop can't answer permission prompts, so run on a branch/worktree you
are willing to let it modify freely.

`effort = "auto"` prevents a global Claude setting from silently making every
slice high-effort: Haiku maps to low, Sonnet to medium, and Opus to high. Set an
explicit `low` / `medium` / `high` / `xhigh` / `max`, or use `inherit` to defer
to Claude settings. A legacy `--effort` in `extra_args` remains authoritative.

## Requirements
- The `claude` CLI on PATH (authenticated).
- The Rust toolchain to build (via the personalize script).

## Development
```bash
cargo test          # backlog/context/config/stream/state/git/thrash
cargo build --release
```
Modules: `backlog` (schema/lint) · `context` (bounded brief) · `config` ·
`stream` (NDJSON) · `classify` · `control` (loop, thrash, budgets, timeout) ·
`state` (`.ralph/`) · `git` · `init` (`ralph init` scaffolding). See
`docs/superpowers/specs/2026-07-17-ralph-rust-design.md` for the original design.
