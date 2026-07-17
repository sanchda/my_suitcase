# ralph — external autonomous loop

Runs `claude -p` in a loop, **fresh context each iteration**, feeding the same
prompt until a completion marker appears. This is the "pure Ralph" (Geoffrey
Huntley) external form — each call starts cold and stays cheap, so it suits
context-expensive / thinking models. Cross-iteration state lives in files, not
context.

This is the Rust runner (`tools/ralph/`, a cargo crate). Beyond looping it adds
**live stream parsing**, **cost / wall-clock budgets**, an **opt-in
per-iteration timeout**, and **no-progress detection** that escalates the model
tier and then aborts. It replaces the previous `ralph.sh`.

## Global tool vs. local driving files

This directory (`$SUITCASE/tools/ralph/`) is the **global, project-agnostic
tool**. Its personalize script builds the binary and installs it to
`~/.local/bin/ralph`, so `ralph` is on your PATH. The crate contains nothing
about any one project.

Everything that *drives* a run is **local to the repo you run it in**:

| Kind | File (default path) | Global or local? |
|------|---------------------|------------------|
| Runner | `ralph` (this tool) | **global** — on PATH |
| Per-iteration prompt | `tools/ralph/PROMPT.md` | **local** (copy `PROMPT.template.md`) |
| North star (optional) | `tools/ralph/VISION.md` | **local** |
| Ordered backlog (optional) | `tools/ralph/BACKLOG.md` | **local** |
| Durable memory / log | `tools/ralph/PROGRESS.md` | **local** |
| Config (optional) | `tools/ralph/ralph.toml` | **local**, committed |
| Runtime (counter, logs, MODEL/STATUS) | `.ralph/` (gitignored) | **local**, generated |

Rule of thumb: **the runner is global; the prompts, config, and record-keeping
are local.**

## Install

Build and install via the suitcase personalize script (requires the Rust
toolchain; `claude` must be authenticated on PATH at runtime):

```bash
$SUITCASE/personalize/scripts/setup_ralph.sh
# or, with everything else: $SUITCASE/personalize/personalize
```

Rebuild after source changes by re-running that script.

## Quick start (in the repo you want worked on)

1. `cp "$SUITCASE/tools/ralph/PROMPT.template.md" tools/ralph/PROMPT.md` and fill
   in every `{{...}}` — the GOAL, the verification command, the commit contract.
2. Optionally add `tools/ralph/VISION.md` and `tools/ralph/BACKLOG.md`, and seed
   `tools/ralph/PROGRESS.md` with the goal + a "Next:" line.
3. Add `.ralph/` to the repo's `.gitignore`.
4. Run it on a dedicated branch:

   ```bash
   ralph --max-iterations 30      # from the repo root
   ```

   Test a single pass first with `ralph --once`.

Run **one `ralph` per worktree** — each loop drives the repo it is launched in.

## Watching / controlling a running loop
- **Live status of the active iteration** (tool, elapsed, output tokens, last
  activity): `cat .ralph/live`
- **Raw stream of the active iteration** (includes thinking): `tail -f .ralph/current.log`
- **High-level progress:** `tail -f .ralph/run.log`
- **Stop gracefully** after the current iteration: `touch .ralph/STOP`
- **Resume** later: just re-run `ralph` — the counter in `.ralph/iteration`
  persists.
- **Launch detached** for overnight runs: `nohup setsid ralph … &`.

## Completion
The loop ends when the model's **final text** (from the result envelope's
`.result`, which excludes thinking) contains the marker token on its own line,
default `RALPH_COMPLETE`. Your `PROMPT.md` must instruct the model to emit it
only when the whole goal is genuinely done and verified.

## Per-iteration hand-offs (the agent writes these)
Each iteration ends by writing two one-word files that steer the next step:

- `.ralph/MODEL` — `haiku` / `sonnet` / `opus`, sizing the NEXT iteration
  (mechanical → haiku, normal → sonnet, hard/repeatedly-failing → opus).
- `.ralph/STATUS` — this iteration's type: `code` (a normal committing
  iteration), or `review`/`plan`/`blocked` for an intentional non-code pass.
  Absent is treated as `code`.

Invalid `MODEL` values are ignored with a warning (never abort). See the PROMPT
template for the exact instructions given to the model.

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
Precedence: **defaults ← `tools/ralph/ralph.toml` ← env (`RALPH_*`) ← flags**.
`ralph.toml` is optional; absent → all defaults. `ralph --help` lists every flag.

Example `tools/ralph/ralph.toml`:

```toml
model = "sonnet"
fallback_model = "sonnet"
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
| `max_iterations` | `RALPH_MAX_ITER` | `--max-iterations` | `0` |
| `max_cost_usd` | `RALPH_MAX_COST` | `--max-cost` | `0` |
| `max_duration` | `RALPH_MAX_DURATION` | `--max-duration` | `0` |
| `iteration_timeout` | `RALPH_ITER_TIMEOUT` | `--iteration-timeout` | `0` |
| `escalate_after` | `RALPH_ESCALATE_AFTER` | `--escalate-after` | `2` |
| `abort_after` | `RALPH_ABORT_AFTER` | `--abort-after` | `4` |
| `marker` | `RALPH_MARKER` | `--marker` | `RALPH_COMPLETE` |
| `prompt` | `RALPH_PROMPT` | `--prompt` | `tools/ralph/PROMPT.md` |
| `dir` | `RALPH_DIR` | `--dir` | `.ralph` |
| `yolo` | `RALPH_YOLO` | `--no-yolo` | `true` |
| `limit_wait` / `_max` | `RALPH_LIMIT_WAIT[_MAX]` | — | 300 / 3600 |
| `transient_wait` / `_max` | `RALPH_TRANSIENT_WAIT[_MAX]` | — | 10 / 300 |
| `extra_args` | `RALPH_EXTRA_ARGS` | — | — |
| `escalation_ladder` | — | — | `["haiku","sonnet","opus"]` |
| — | `RALPH_CONFIG` | `--config` | `tools/ralph/ralph.toml` |
| — | — | `--once` | run one iteration then exit |

`--dangerously-skip-permissions` is on by default (`--no-yolo` disables) — an
unattended loop can't answer permission prompts, so run on a branch/worktree you
are willing to let it modify freely.

## Requirements
- The `claude` CLI on PATH (authenticated).
- The Rust toolchain to build (via the personalize script).

## Development
```bash
cargo test          # classify / config / stream / state / git / thrash
cargo build --release
```
Modules: `config` · `stream` (NDJSON) · `classify` · `control` (loop, thrash,
budgets, timeout) · `state` (`.ralph/`) · `git`. See
`docs/superpowers/specs/2026-07-17-ralph-rust-design.md` for the design.
