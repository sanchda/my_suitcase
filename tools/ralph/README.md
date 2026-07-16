# ralph — external autonomous loop

Runs `claude -p` in a `while` loop, **fresh context each iteration**, feeding the
same prompt until a completion marker appears. This is the "pure Ralph" (Geoffrey
Huntley) external form — distinct from any in-session `ralph-loop` plugin, and
better suited to context-expensive / thinking models because each call starts
cold and stays cheap. Cross-iteration state lives in files, not context.

## Global tool vs. local driving files

This directory (`$SUITCASE/tools/ralph/`) is the **global, project-agnostic
tool**. `bin/ralph` puts it on your `PATH`. The script contains nothing about any
one project.

Everything that *drives* a run is **local to the repo you run it in**:

| Kind | File (default path) | Global or local? |
|------|---------------------|------------------|
| Runner | `ralph` (this tool) | **global** — on PATH |
| Per-iteration prompt | `tools/ralph/PROMPT.md` | **local** (copy `PROMPT.template.md`) |
| North star (optional) | `tools/ralph/VISION.md` | **local** |
| Ordered backlog (optional) | `tools/ralph/BACKLOG.md` | **local** |
| Durable memory / log | `tools/ralph/PROGRESS.md` | **local** |
| Runtime (counter, logs, MODEL) | `.ralph/` (gitignored) | **local**, generated |

Rule of thumb: **the script is global, the prompts and record-keeping are local.**

## Quick start (in the repo you want worked on)

1. `cp "$SUITCASE/tools/ralph/PROMPT.template.md" tools/ralph/PROMPT.md` and fill
   in every `{{...}}` — the GOAL, the verification command, the commit contract.
2. Optionally add `tools/ralph/VISION.md` (principles/guardrails) and
   `tools/ralph/BACKLOG.md` (ordered work), and seed `tools/ralph/PROGRESS.md`
   with the goal + a "Next:" line.
3. Add `.ralph/` to the repo's `.gitignore`.
4. Run it on a dedicated branch:

   ```bash
   ralph --max-iterations 30      # from the repo root
   ```

   Test a single pass first with `ralph --once`.

## Watching / controlling a running loop
- **Live view of the active iteration** (includes thinking, since the log is
  `stream-json`): `tail -f .ralph/current.log`
- **High-level progress:** `tail -f .ralph/run.log`
- **Stop gracefully** after the current iteration: `touch .ralph/STOP`
- **Resume** later: just re-run `ralph` — the counter in `.ralph/iteration`
  persists, so it picks up where it left off.
- **Launch detached** for overnight runs: `nohup setsid ralph … &` (export any
  env the verification step needs, e.g. tool paths, before launching).

## Completion
The loop ends when the model's **final text** (not thinking — the harness reads
the envelope's `.result`, which excludes thinking) contains the marker token on
its own line, default `RALPH_COMPLETE`. Your `PROMPT.md` must instruct the model
to emit it only when the whole goal is genuinely done and verified.

## Robustness against running out of usage credits
The Claude CLI returns **exit 0 even on API errors**, so the harness ignores exit
codes and parses the JSON result envelope (`is_error`, `api_error_status`,
`.result`). Errors are classified:

| Class | Trigger | Behavior |
|-------|---------|----------|
| **LIMIT** | 429, or text matching `usage limit` / `credit balance` / `quota` / `will reset` / `rate limit` | Wait it out. Unlimited retries, **capped exponential backoff** (base `RALPH_LIMIT_WAIT`=300s, cap `RALPH_LIMIT_WAIT_MAX`=3600s). Same iteration retried — no work lost. |
| **TRANSIENT** | 5xx / `overloaded` / network / timeout / empty output (crash) | Short capped backoff (base 10s, cap 300s), unlimited retries. |
| **FATAL** | 401/403 auth, 400/404 bad model / invalid request | Abort with a clear message — looping won't fix config. |

So if you run out of credits mid-run, the loop parks on that iteration, sleeps,
and keeps retrying (5m → 10m → … → capped 60m) until your quota resets, then
continues automatically. Nothing is lost. The full error text is logged to the
iteration log and `.ralph/last-result.json` so you can tune the patterns — edit
`classify()` in `ralph.sh` if your plan's wording differs.

## Per-iteration model sizing
Each iteration ends by writing `haiku` / `sonnet` / `opus` into `.ralph/MODEL`,
sizing the model for the NEXT step (mechanical → haiku, normal → sonnet, hard
design/refactor or repeated failure → opus). The harness reads it via
`resolve_model()` before each `claude` call; invalid contents are ignored with a
warning (never aborts). The chosen tier is logged per-iteration in `run.log`, so
cost creep is visible. You can also drop a value in by hand while the loop runs to
steer the next step.

## Commits (legible incremental history)
Run on a dedicated branch; the PROMPT tells the agent to **commit once per
verified iteration**, so history reads as one clean step per commit. The prompt
must instruct it to:
- stage only the files it changed this iteration, by explicit path (never
  `git add -A`, since repos usually have unrelated untracked files);
- commit only when verification passed;
- never `git reset` / rebase / amend / switch branches — only add new commits.

The harness logs a `⚠ … newly-dirty` warning if the tracked tree is still dirty
after a successful iteration, so a forgotten commit is visible in `run.log`.

## Configuration (env vars, or flags)
| Env | Flag | Default | Meaning |
|-----|------|---------|---------|
| `RALPH_MODEL` | `--model` | `sonnet` | Default model tier. |
| `RALPH_FALLBACK_MODEL` | `--fallback-model` | `sonnet` | Overloaded-fallback (`""` disables; skipped when equal to the resolved model). |
| `RALPH_MAX_ITER` | `--max-iterations` | `0` (unlimited) | Hard iteration cap. |
| `RALPH_MARKER` | `--marker` | `RALPH_COMPLETE` | Completion token. |
| `RALPH_PROMPT` | `--prompt` | `tools/ralph/PROMPT.md` | Prompt file. |
| `RALPH_DIR` | `--dir` | `.ralph` | Runtime/log dir. |
| `RALPH_YOLO` | `--no-yolo` disables | `1` | Passes `--dangerously-skip-permissions` (required for unattended runs). |
| `RALPH_OUTPUT_FORMAT` | — | `stream-json` | `json` for terse logs; `stream-json` captures thinking. |
| `RALPH_LIMIT_WAIT` / `_MAX` | — | 300 / 3600 | Usage-limit backoff base / cap (s). |
| `RALPH_TRANSIENT_WAIT` / `_MAX` | — | 10 / 300 | Transient backoff base / cap (s). |
| `RALPH_EXTRA_ARGS` | — | — | Extra flags passed to `claude` verbatim. |

## Requirements
- `claude` CLI on PATH (authenticated), and `jq`.

## Caveats
- `--dangerously-skip-permissions` is on by default (unattended loops can't answer
  prompts). Run on a branch/worktree you're willing to let it modify freely.
- To hard-kill a runaway loop: Ctrl-C the runner (it traps SIGINT and exits), or
  `touch .ralph/STOP` for a graceful stop after the current iteration. Because
  every iteration commits, `git log`/`git revert` gives clean rollback points.
