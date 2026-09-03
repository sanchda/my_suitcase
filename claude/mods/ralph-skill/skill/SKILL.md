---
name: ralph
description: Use when setting up, running, steering, or debugging a `ralph` autonomous loop in a repo — running `ralph init`, filling `.ralph/PROMPT.md`, authoring or mutating `BACKLOG.md`, reading `queued (applies at the next iteration boundary)`, a loop that stalled/aborted/escalated/won't start, `.ralph/` state files, model tiers, budgets, or starting the next arc.
---

# Driving a ralph loop

You are the **outer, interactive** Claude helping a human set up, operate, and debug loops. You are
not the agent inside an iteration — that one gets `.ralph/PROMPT.md` and often runs under
`--safe-mode`, which omits skills entirely. Don't write in-loop iteration instructions.

## Mental model

- `ralph` is a **global** binary on PATH. Everything that drives a run is **local**:
  `.ralph/{PROMPT.md,BACKLOG.md,PROGRESS.md,VISION.md,ralph.toml}` plus runtime state. The whole
  `.ralph/` tree is gitignored — never product.
- Each iteration is a **fresh** `claude -p` process. Nothing carries over except what the runner
  injects: base prompt + `## Learnings` + a linted contract + the resolved leaf's excerpt + the
  previous turn's carry-forward. Cross-iteration state lives in **files**, not context.
- **One loop per repo**, held by `.ralph/loop.pid`. A second `ralph` here exits 2 naming the live
  pid; a pidfile left by a killed loop is reclaimed on the next start.
- Run on a **dedicated branch**: `--dangerously-skip-permissions` is on by default (`--no-yolo`
  disables) and a branch switch mid-run is a fatal abort.

**The exhaustive reference lives in the binary and can never drift from the installed version.**
Point the reader there rather than paraphrasing: `ralph --help` (every flag + default),
`ralph schema` (backlog format), `ralph hints` (prompt-authoring lessons), `ralph <sub> --help`.

Driving loops from Discord is a separate binary with its own skill — use **ralphd** for the
bridge, its config, and its traps. Nothing here depends on it.

## Setting up a new loop

```bash
cd <repo> && git switch -c ralph/<goal>   # dedicated branch first
ralph init                                # idempotent; never overwrites
```

`init` writes `PROMPT.md`, `ralph.toml`, `BACKLOG.md`, `VISION.md`, `PROGRESS.md`, `archive/`, and
appends a `/.ralph/` block to `.gitignore`. The stub `BACKLOG.md` **deliberately fails lint** (its
`Verify:` is a `{{...}}` placeholder) — that is the signal to author it, not a bug.

Then, in order:

1. Fill **every** `{{...}}` in `.ralph/PROMPT.md`: GOAL, the "Done" condition, exact verification
   commands + success markers, `{{BRANCH_NAME}}`, commit-trailer policy. Read `ralph hints` first —
   it is the distilled lesson set for exactly this step.
2. Author the backlog through the CLI (below). `VISION.md` is optional; leave `PROGRESS.md` alone.
3. `ralph lint` (exit 0 clean, 1 schema errors; warnings allowed), then `ralph brief` to see the
   exact bounded context the model will receive.
4. `ralph --once` — one real pass — before committing to a run.
5. `ralph --max-iterations 30`, or detached: `nohup setsid ralph … &`.

Ralph re-lints before **every** iteration and refuses to launch while schema errors remain.

## Backlog: never hand-edit it

The running loop owns `BACKLOG.md`; a hand edit is silently overwritten and unprotected. The CLI is
the **only** schema-checked path — each mutation is linted in memory and rejected before it touches
disk:

```bash
ralph add "<title>" --verify "<cmd>"            # next free top-level id
ralph add --under 3 "<title>" --verify "<cmd>"  # next free 3.N stage
ralph add 3.1.1 "<title>" --verify "<cmd>"      # explicit id; parent 3.1 must exist
ralph done 3.1                                  # check off (never cascades)
ralph uncheck 3.1                               # reopen, plus ancestors it closed
ralph drop 3 --recursive                        # remove a subtree (archived, not deleted)
```

`ralph backlog add …` / `ralph backlog edit …` are older flag-style aliases on the same path.

Omit `--verify` and pipe a full multi-line body (prose + a `Verify:` line) on stdin instead. A
duplicate id exits **3**, not a lint dump. `drop` archives what it removed to
`.ralph/archive/dropped-<ts>.md` and refuses the leaf a running loop selected.

### v2 schema essentials (full detail: `ralph schema`)

- Starts with `<!-- ralph-backlog: v2 -->`. Absent → warning + lenient compatibility mode.
- `- [ ] **<id> — <title>.**` — `-` bullet only, em dash ` — ` between id and title, ids limited to
  letters/digits/`.`/`_`/`-`, titles may not contain `**`.
- Children indent **exactly two spaces** per level; their id must be `<parent>.`-prefixed.
- Every **pending** task — staged parents included — needs a non-placeholder `Verify:` (empty,
  `{{…}}`, `TODO`, `TBD`, `replace me` all fail). A parent's `Verify:` is its closure gate.
- Task-looking examples go inside fenced code blocks; fences are invisible to the parser (an
  unclosed one is an error).

### The `@tier` slot — position-anchored, exact

```markdown
- [ ] **12 — Rework the shared base.** @opus — big, cross-cutting change.
  Verify: cargo test
```

Immediately after the label's closing `**`, then ` — ` (em dash U+2014 + space) before the prose.
Only `@haiku`/`@sonnet`/`@opus`, at most one, only in that slot. A misspelling, an en dash, an ASCII
hyphen, a missing space, two tiers, or a leftover v1 `(opus/…)` parenthetical in a pending task's
body is a **hard lint error** that refuses the iteration — never a silent fallback. Consequence:
task prose may not begin with `@` — rewrite `**1 — Notify.** @channel — …` so `@` is not first. A
tier valid in schema but absent from `escalation_ladder` runs the default model and logs
`⚠ task declares @X, absent from escalation_ladder`.

### Selection order

First unchecked task with **no unchecked descendants**, in document order. A parent with pending
children is a container; once they all close it becomes its own integration/closure step. `done`
never cascades — checking a parent with pending stages is rejected. `uncheck` reopens the leaf *and*
any ancestor its check-off closed. A checked task after a pending sibling is a warning only.

## `queued` is SUCCESS, not failure

No CLI mutation writes `BACKLOG.md` in place. Each writes a request into `.ralph/inbox/`:

- **Loop running** → prints `queued (applies at the next iteration boundary)`, exit 0, file unchanged. The loop drains at the top of the next iteration, before routing. This is the designed behavior — it is what keeps the backlog from shifting under a running agent.
- **No loop running** → the command drains for itself immediately, printing e.g. `added task 3`.

A request that cannot be replayed at drain time is parked in `.ralph/inbox/rejected/` with its full JSON and reported to `run.log`; enqueue-time linting catches nearly everything first.

## Operating a live loop

```bash
ralph status            # iter N · M pending · current: <id — title>, plus next few
ralph status --json     # one machine-readable line
cat .ralph/live         # the active iteration: tool, elapsed, output tokens, last text
tail -f .ralph/current.log   # raw NDJSON stream of the active iteration (incl. thinking)
tail -f .ralph/run.log       # one line per iteration + perf + warnings
```

`ralph stop` writes `.ralph/STOP`, honored at the **next boundary** (a long turn finishes first) and suppresses `--restart`. `ralph stop --now` also SIGTERMs the loop, tearing down the `claude` session group — the point, since `claude` leads its own session and a naive kill orphans its whole subprocess tree. **Resume** by re-running `ralph`; `.ralph/iteration` persists.

`ralph msg "<text>"` drives a persistent steering session (`.ralph/msg-session`) that reads status and queues work rather than implementing it. `--model` is **sticky**: it repins the thread in `.ralph/msg-model` until changed or `--new`, and the active model prints to stderr every call — if a pin seems stuck, `cat .ralph/msg-model` is the whole story. One `msg` at a time; a second is refused, not queued.

## Diagnosing

**The no-progress streak.** An iteration is *no-progress* when it was a `code` pass that made no new commit, when a commit landed but breached the git contract, when the judge refuted it, or when it was a transient/timeout retry. `plan`/`review` passes are excluded. Defaults: `escalate_after = 2` (step one tier up `haiku → sonnet → opus`), `abort_after = 4` (halt). A productive `code` pass resets both the streak and the escalation.

**`blocked` is not "needs a bigger model."** A `blocked` pass never escalates and aborts after **2 consecutive** ones. If a bigger tier would solve it, that is a misuse — fix it with a `@tier` decoration or `ralph model <tier>`. Reserve `blocked` for approval gates, missing credentials, or the same failure surviving escalation.

**Mechanical git contract audit** — after every successful iteration the runner enforces with git what the prompt only requests (lenient outside a git repo):

| Finding | Consequence |
|---|---|
| Branch switched | **Fatal** — logged, posted, loop aborts immediately (exit 1) |
| HEAD moved without fast-forward (amend/reset/rebase) | counts as no-progress |
| Any `.ralph/` path in the new commits | counts as no-progress |
| Tracked tree still dirty vs. the loop-start baseline | `⚠ N newly-dirty` warning only |

**Error classification** — the Claude CLI returns **exit 0 even on API errors**, so ralph ignores exit codes entirely and parses the result envelope (`is_error`, `api_error_status`, `.result`), persisted to `.ralph/last-result.json`. Anything unrecognized defaults to TRANSIENT.

| Class | Trigger | Behavior |
|---|---|---|
| LIMIT | 429, or text like `usage limit` / `credit balance` / `quota` / `rate limit` / `will reset` | unlimited retries, 300s → 3600s backoff, **never** no-progress |
| TRANSIENT | 500/502/503/504/529, `overloaded`/network/timeout, or no envelope at all (crash, kill, timeout) | 10s → 300s backoff, retried, **counts** as no-progress |
| FATAL | 401/403/400/404, auth failure, bad model | abort with a clear reason |

**Adversarial judge (opt-in, off by default).** `judge_tiers = ["opus"]` in `ralph.toml` gives every committed `code` iteration that *ran* on a listed tier a one-shot second opinion on `judge_model` (default `sonnet`), told to refute if uncertain. On refute the runner mechanically un-checks the leaf (and ancestors it closed) so routing re-selects it, and counts the iteration as no-progress. The harness fails **open**: a missing or garbled judge call passes.

## Model precedence — exactly, highest first

1. Escalation override (from the no-progress streak)
2. One-shot `.ralph/MODEL` — written by `ralph model <tier>` or a HANDOFF `model` field; consumed
   and cleared on read. Must be on the configured `escalation_ladder`.
3. The resolved leaf's own `@tier` decoration
4. The run default (`--model` / `model`, default `sonnet`)

Effort follows: `effort = "auto"` (default) maps haiku→low, sonnet→medium, opus→high; `inherit` defers to Claude settings; an explicit level or an `--effort` in `extra_args` wins. Config precedence overall is **defaults ← `.ralph/ralph.toml` ← `RALPH_*` env ← flags**, with these keys TOML-only (no flag): `synth_model`, `judge_tiers`, `judge_model`, `escalation_ladder`, `limit_wait[_max]`, `transient_wait[_max]`, `extra_args`, `budget_usd`, `budget_window`. Tier lists accept only `haiku`/`sonnet`/`opus`; anything else fails config validation.

## Budgets and spend

All checked at iteration boundaries; each halts the loop when hit.

| Budget | Where | Default |
|---|---|---|
| `--max-cost` / `RALPH_MAX_COST` | in-process total — a restart hands the loop a **fresh** allowance | 0 (off) |
| `budget_usd` + `budget_window` | TOML-only; sums `.ralph/ledger.jsonl` over a trailing window, so it **survives restarts** | 0 (off) |
| `--max-duration`, `--max-iterations` | wall clock, iteration count | 0 (off) |
| `--iteration-timeout` | SIGKILLs the iteration's whole process group; treated as a transient retry | 0 (off) |

`.ralph/ledger.jsonl` gets one `{"ts","iter","model","cost_usd"}` line per iteration — that file is where spend actually lives. The synth, judge, and `ralph learn` calls are extra model calls whose cost is **not** counted toward `max_cost_usd` or the ledger. Set `iteration_timeout` whenever the work launches engines, servers, or browsers: ralph SIGKILLs the child's process group after every iteration, but one runaway iteration can still OOM the box.

## Arc lifecycle

The run completes only when the final result text contains the marker (`RALPH_COMPLETE`, on its own line) **and** the backlog has no pending task. A marker with work still pending is logged as `⚠ completion marker ignored` and the loop continues.

Completion closes the arc: `BACKLOG.md` → `.ralph/archive/BACKLOG-<ts>.md`; the carry-forward → `PROGRESS-<ts>.md` and `PROGRESS.md` cleared; the msg session id archived; the iteration counter reset to 0 so a fresh `--max-iterations` means what it says. **`.ralph/learnings/` is deliberately untouched** — that is the memory meant to survive arcs, as are `PROMPT.md`, `ralph.toml`, and the webhook. Start the next arc from `ralph add` alone: with the backlog file absent, `add` bootstraps a fresh schema-valid one. Edit `PROMPT.md` between arcs only if the goal or contract changed.

Separately, after each successful iteration the runner sweeps fully-completed *leading* top-level sections into `.ralph/archive/BACKLOG-completed.md`. A shrinking `BACKLOG.md` is normal curation, not lost work.

## `ralph learn` — propose, then approve

```bash
ralph learn              # mine run.log + carry-forward → numbered proposals (saved, NOT applied)
ralph learn --apply      # write all → .ralph/learnings/<slug>.md
ralph learn --apply 1,3  # write a subset
ralph learn --discard    # drop the saved proposals
```

Existing learnings are shown to the miner so they are not re-proposed, and "no non-obvious learnings found" is the **correct** outcome for a clean run, not a failure. One learning per file, so pruning is `rm`. They are injected into every iteration under `## Learnings`, capped at 1 KB per file and 4 KB total; over-budget files are named in the prompt rather than silently dropped.

## GOTCHAS

| Trap | Reality |
|---|---|
| Hand-editing `BACKLOG.md` (or a check-off with Edit) | The loop owns the file; hand edits are silently overwritten. Use `ralph add/done/uncheck/drop`. |
| Reading `queued (applies at the next iteration boundary)` as a failure | That is success. Exit 0. Do not retry, do not "fix" it by editing the file. |
| Writing `PROGRESS.md` or a `Next:` line | Runner-owned. It overwrites `PROGRESS.md` with a synthesized carry-forward every iteration, and there is no `Next:` parsing — routing comes from the backlog alone. |
| Starting a second loop in one repo | Refused, exit 2, naming the live pid. Use a separate worktree, or `ralph stop` first. |
| `git add -A` / `git add .` in the prompt or by hand | Stages the untracked `.ralph/` working set; committed `.ralph/` paths count as no-progress. Stage product paths by explicit path. |
| Treating ralph's or claude's exit code as the outcome | The CLI returns 0 on API errors. Read `run.log`, `.ralph/last-result.json`, or `ralph status`. |
| A stage added mid-iteration missing from a following `ralph lint` | Correct — it applies at the next boundary. |
| Assuming `ralph init` leaves a runnable backlog | Its stub `Verify:` is a placeholder that intentionally fails lint. |
| Using `-` or `–` instead of ` — ` in a label or after `@tier` | Hard lint error; the runner refuses the iteration. The error message names the character it found. |
| Expecting `ralph done <parent>` to cascade | Rejected while stages are pending. The parent is its own closure step. |
| Expecting `--restart` to always come back | Only an **ungraceful** signal death restarts. SIGINT/TERM/HUP/QUIT, a pending STOP, completion, abort, and 5 rapid (<60s) crashes are all terminal. |
| Forgetting that `extra_args = ["--safe-mode", …]` omits skills, hooks, plugins, MCP, and memory | Then `PROMPT.md` must carry every project and verification rule itself. |
