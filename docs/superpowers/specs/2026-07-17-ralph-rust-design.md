# Ralph in Rust — design

**Date:** 2026-07-17
**Status:** approved, implementing
**Replaces:** `tools/ralph/ralph.sh` (deleted on parity)

## Goal

Migrate the Ralph external autonomous loop from bash (`tools/ralph/ralph.sh`)
to a Rust binary, mirroring the existing `plan-vim-gate` crate in this repo. The
migration is a **capability upgrade**, not a transliteration: the Rust runner
adds live stream parsing, cost/wall-clock budgets, an opt-in per-iteration
timeout, and no-progress/thrash detection with model escalation — things bash
makes painful or impossible.

Non-goals: parallel multi-worktree orchestration inside one process. The
concurrency model is **one `ralph` process per worktree**, so the runner stays a
single synchronous loop.

## What stays the same

The "brain" of Ralph is unchanged. Cross-iteration state lives in files, each
iteration is a fresh `claude -p` process fed the same prompt, and the driving
files remain local, per-project markdown:

- `tools/ralph/PROMPT.md` (copied from `PROMPT.template.md`)
- `tools/ralph/VISION.md`, `BACKLOG.md`, `PROGRESS.md`
- Completion still signalled by a marker token (`RALPH_COMPLETE`) on its own
  line in the model's final text.
- `.ralph/` remains the gitignored runtime dir.

## Architecture

A cargo crate at `tools/ralph/` (replacing `ralph.sh` in place; `target/`
gitignored), styled after `plan-vim-gate`: `std`-first, small dependency set,
synchronous.

### Modules

| Module | Responsibility | Depends on |
|---|---|---|
| `main.rs` | Load config, run the control loop, set process exit code. | all |
| `config.rs` | Merge defaults ← `tools/ralph/ralph.toml` ← env (`RALPH_*`) ← flags; validate; parse duration suffixes (`8h`, `30m`, `300s`). | `serde`, `toml` |
| `stream.rs` | Consume `claude` NDJSON stdout line-by-line: tee each raw line to the iteration log **and** parse events to update a live `IterStatus`; return the final result envelope. | `serde_json` |
| `classify.rs` | Pure fn `(is_error, api_status, text) -> Class` (SUCCESS / LIMIT / TRANSIENT / FATAL). Fully unit-tested. | — |
| `control.rs` | The loop: spawn, watchdog timeout, backoff, thrash streak + escalation, budget checks, STOP file. | `stream`, `classify`, `git`, `state` |
| `state.rs` | Read/write `.ralph/` runtime files (counter, `MODEL`, `STATUS`, live `status`, `last-result.json`, `run.log`). | — |
| `git.rs` | Loop-start baseline; per-iteration productivity check (did HEAD advance / tracked tree change this iteration?). | — |

### Dependencies

`serde` + `serde_json` (already used by `plan-vim-gate`) and `toml`. No async
runtime, no `signal-hook` (see Signals). `[profile.release] opt-level = "s"` to
match the existing crate.

## File layout

**Committed, per-project (the brain — unchanged, plus config):**
`tools/ralph/PROMPT.md`, `VISION.md`, `BACKLOG.md`, `PROGRESS.md`, and the new
`tools/ralph/ralph.toml` (version-controlled per project).

**Runtime, gitignored (`.ralph/`):**

| File | Writer | Purpose |
|---|---|---|
| `iteration` | ralph | Persisted iteration counter. |
| `MODEL` | agent | Next iteration's model tier (`haiku`/`sonnet`/`opus`). |
| `STATUS` | agent | This iteration's type: `code` / `review` / `blocked` / … |
| `live` | ralph | Live per-iteration status (tool, elapsed, tokens, last activity). NEW. Named `live`, not `status`, so it can't collide with `STATUS` on case-insensitive (macOS) filesystems. |
| `current.log` → `logs/iter-NNNN-*.log` | ralph | Full raw `stream-json` for `tail -f`. |
| `last-result.json` | ralph | Last result envelope. |
| `run.log` | ralph | High-level per-iteration progress. |
| `STOP` | operator | Graceful-halt request. |
| `git-baseline` | ralph | Tracked-tree baseline at loop start. |

Config that should be version-controlled lives in the committed
`tools/ralph/ralph.toml`, never in gitignored `.ralph/`.

## Control flow (per iteration)

```
loop:
  if STOP file present            -> halt (graceful), remove STOP
  check budgets (cost, wall-clock, max-iter) -> halt with reason
  record git HEAD (for productivity check)
  model = escalation_override OR .ralph/MODEL OR config.model
  spawn: claude -p --output-format stream-json --verbose \
         [--dangerously-skip-permissions] --model <model> \
         [--fallback-model <fb>] <extra_args>  < PROMPT.md
  stream::consume(child, iter_log):
     for each stdout line:
        append raw line to logs/iter-NNNN.log
        parse -> update .ralph/status (current tool, elapsed, running cost, last activity)
        keep the line whose "type" == "result"
     (if iteration_timeout > 0: watchdog thread kills child at deadline)
     -> Option<ResultEnvelope>   (None if killed/crashed/empty)
  classify -> Class ; dispatch
```

### Dispatch by class

| Class | Action |
|---|---|
| **SUCCESS** | Parse envelope (`is_error=false`). Read `.ralph/STATUS`. Run git productivity check. Advance counter; reset backoffs. If final text contains the marker on its own line → COMPLETE, halt. Feed productivity + STATUS into the thrash tracker. |
| **LIMIT** | Usage/rate limit. Capped exponential backoff (base `limit_wait` 300s, cap `limit_wait_max` 3600s), unlimited retries, same iteration. |
| **TRANSIENT** | 5xx / network / crash / empty-envelope / **killed-by-timeout**. Short capped backoff (base 10s, cap 300s), unlimited retries. A timeout kill also records a timeout strike (feeds thrash). |
| **FATAL** | Auth (401/403) / bad model / bad request (400/404). Abort immediately with a clear message. |

Exit code (unreliable from the CLI) is ignored; classification is driven by the
parsed result envelope, exactly as today.

## Thrash detection & escalation (new core)

A **progress streak** counts consecutive *unproductive* iterations.

An iteration counts as **no-progress** when any of:
1. `STATUS=code` (or STATUS absent, treated as `code`) but the git productivity
   check shows no new commit / no tracked-file change this iteration; or
2. it repeats the same classified failure as the previous iteration; or
3. it was killed by the per-iteration timeout (a timeout strike).

An iteration is **excluded** from the streak (decorated as a legit
non-code pass) when `STATUS` is a non-`code` type (`review`, `blocked`, …).
These are logged with a distinct tag in `run.log`, neither incrementing nor
resetting the streak.

Actions:
- streak ≥ `escalate_after` (default **2**): escalate the model one tier up the
  ladder `haiku → sonnet → opus` for the next attempt, overriding `.ralph/MODEL`;
  log the escalation.
- streak ≥ `abort_after` (default **4**): abort the loop with a clear reason
  ("no progress after N iterations; escalated to opus and still stuck").

A productive iteration resets the streak and clears the escalation override.

## Budgets

Checked at iteration boundaries (reliable cost comes from the result envelope at
iteration end):

| Budget | Config | Default | On hit |
|---|---|---|---|
| Cumulative cost | `max_cost_usd` | 0 (off) | Halt with reason. |
| Wall-clock | `max_duration` | 0 (off) | Halt with reason. |
| Iterations | `max_iterations` | 0 (off) | Halt (existing). |

Cumulative cost sums `total_cost_usd` across iterations. Wall-clock measures from
loop start. Only the opt-in per-iteration timeout hard-kills mid-iteration.

## Per-iteration timeout

Off by default (`iteration_timeout = 0`). When set, a watchdog thread kills the
`claude` child at the deadline. A killed iteration produces no result envelope →
classified TRANSIENT (short backoff, retry) **and** records a timeout strike that
feeds thrash detection, so repeated timeouts escalate then abort.

## Live observability

`stream.rs` parses the NDJSON stream as it arrives and writes a compact,
human-readable `.ralph/live` (current tool, elapsed, output tokens, last text),
refreshed live. The full raw `stream-json` is still
tee'd to `logs/iter-NNNN-*.log` (symlinked as `current.log`) for `tail -f` and
deep debugging. `run.log` keeps the terse per-iteration high-level lines.

## Configuration

Precedence: **defaults ← `tools/ralph/ralph.toml` ← env (`RALPH_*`) ← flags**.

| Key (toml) | Env | Flag | Default | Meaning |
|---|---|---|---|---|
| `model` | `RALPH_MODEL` | `--model` | `sonnet` | Default model tier. |
| `fallback_model` | `RALPH_FALLBACK_MODEL` | `--fallback-model` | `sonnet` | Overloaded-fallback (empty disables; skipped when equal to resolved model). |
| `max_iterations` | `RALPH_MAX_ITER` | `--max-iterations` | `0` | Iteration cap (0 = unlimited). |
| `marker` | `RALPH_MARKER` | `--marker` | `RALPH_COMPLETE` | Completion token. |
| `prompt` | `RALPH_PROMPT` | `--prompt` | `tools/ralph/PROMPT.md` | Prompt file. |
| `dir` | `RALPH_DIR` | `--dir` | `.ralph` | Runtime dir. |
| `yolo` | `RALPH_YOLO` | `--no-yolo` | `true` | `--dangerously-skip-permissions`. |
| `output_format` | `RALPH_OUTPUT_FORMAT` | — | `stream-json` | Kept `stream-json` for live parsing. |
| `limit_wait` / `_max` | `RALPH_LIMIT_WAIT[_MAX]` | — | 300 / 3600 | Usage-limit backoff base/cap (s). |
| `transient_wait` / `_max` | `RALPH_TRANSIENT_WAIT[_MAX]` | — | 10 / 300 | Transient backoff base/cap (s). |
| `extra_args` | `RALPH_EXTRA_ARGS` | — | — | Extra flags passed to `claude` verbatim. |
| `max_cost_usd` | `RALPH_MAX_COST` | `--max-cost` | `0` | Cumulative cost cap (0 = off). NEW. |
| `max_duration` | `RALPH_MAX_DURATION` | `--max-duration` | `0` | Wall-clock cap; accepts `8h`/`30m`/`300s`. NEW. |
| `iteration_timeout` | `RALPH_ITER_TIMEOUT` | `--iteration-timeout` | `0` | Per-iteration timeout (0 = off). NEW. |
| `escalate_after` | `RALPH_ESCALATE_AFTER` | `--escalate-after` | `2` | No-progress streak before escalating. NEW. |
| `abort_after` | `RALPH_ABORT_AFTER` | `--abort-after` | `4` | No-progress streak before aborting. NEW. |
| `escalation_ladder` | — | — | `["haiku","sonnet","opus"]` | Escalation tiers. NEW. |
| — | — | `--once` | — | Single iteration then exit (testing). |

`ralph.toml` is optional; absent → all defaults. Invalid `.ralph/MODEL` /
`STATUS` values are ignored with a warning (never abort the loop).

## Signals

No custom signal handler (avoids a dependency). Interactive `Ctrl-C` delivers
SIGINT to the foreground process group, killing both `ralph` and its `claude`
child. Detached runs (`nohup setsid ralph &`) are stopped via the `STOP` file
(graceful) or by killing the process group (hard). This matches today's
operational contract.

## Error handling

- CLI exit code ignored; classification from the parsed envelope.
- Missing result envelope (crash / kill / empty output) → TRANSIENT.
- Malformed NDJSON lines are tee'd raw and skipped for parsing (never crash the
  loop).
- FATAL → non-zero process exit with a clear message.
- Config validation errors → exit before the loop with a clear message.

## Testing

- `classify.rs`: unit tests per class, porting every bash regex/status case
  (usage-limit wording, 429, 5xx, 401/403, 400/404, overloaded/network,
  default-transient).
- `config.rs`: precedence/merge, TOML parse, duration-suffix parsing, invalid
  values rejected.
- `stream.rs`: NDJSON fixtures → assert extracted envelope, `IterStatus`, and
  whole-line marker detection (marker in prose must not match).
- `state.rs`: `MODEL`/`STATUS` validation (invalid ignored with warning).
- Thrash tracker: pure function over a sequence of iteration outcomes →
  assert escalate/abort transitions and non-code exclusion.
- `git.rs`: thin; productivity check exercised via a temp git repo in an
  integration test where practical.

## Migration & install

- New crate at `tools/ralph/` (`Cargo.toml`, `src/`, `tools/ralph/.gitignore`
  with `/target`).
- Delete `tools/ralph/ralph.sh` and the `bin/ralph` shim once at parity.
- Add `personalize/scripts/setup_ralph.sh` mirroring
  `setup_plan_vim_gate.sh`: build `--release`, install to `~/.local/bin/ralph`.
  Wire into `personalize/personalize`.
- Update `tools/ralph/README.md` for the new config file and capabilities.
- Update `tools/ralph/PROMPT.template.md`: add a step writing `.ralph/STATUS`
  each iteration (`code` for normal work; `review`/`blocked` for non-code
  passes) alongside the existing `.ralph/MODEL` step.

## Open risks

- Mid-stream cost is an estimate (only the envelope's `total_cost_usd` is
  authoritative); the live `.ralph/status` cost is labelled approximate.
- Escalation ladder assumes `haiku < sonnet < opus`; a custom `ralph.toml`
  ladder is honored verbatim.
