# Ralph: the CLI as the control surface

Make `ralph`'s own CLI the complete control surface for a loop, so that every
capability reachable from Discord is reachable from a terminal, and `ralphd` is
never load-bearing. Six coupled changes: (1) **ralph owns its pidfile**, (2) a
**single-writer inbox** for backlog mutations, (3) **`add`/`drop`/`uncheck`** as
first-class schema-safe subcommands, (4) **`model`** and (5) **`stop --now`**
closing the last two parity gaps, and (6) **`msg`**, a persistent session that
steers the loop through that same CLI.

**Date:** 2026-08-16
**Component:** `~/suitcase/tools/ralph` (the loop runner); `~/suitcase/tools/ralphd` consumes it
**Status:** Design approved, pending implementation plan

## Problem

`ralphd` was built as a Discord adapter over the `ralph` CLI — it shells out for
status (`ralph.rs:45`), stop (`:50`), and backlog add/edit (`:54`, `:58`). Six of
its seven commands mirror a CLI subcommand by construction. But the mirror has
eroded, and the erosion matters now for two reasons: we are moving to many loops
(one Discord channel per repo), and some environments cannot run a Discord bot at
all, so the CLI has to be sufficient on its own.

Four concrete gaps:

**1. `/model` reaches around the CLI.** ralphd writes `.ralph/MODEL` directly
(`ralph.rs:65`) because `ralph` has no `model` subcommand — `main.rs:103-125`
dispatches `init`, `schema`, `hints`, `status`, `backlog`, `learn`, `brief`,
`lint`, `stop`, `start` and nothing else. A terminal user must know the file
format.

**2. There is no way to add a child stage.** `apply_add` (`backlog_edit.rs:40`)
computes its id from `next_top_level_id` (`:28`) — max top-level integer + 1 —
and appends at indent 0. Adding `3.1.1` is impossible through the CLI, so any
agent that wants to queue a sub-stage must hand-edit `BACKLOG.md`. There is also
no way to remove a task at all.

**3. `BACKLOG.md` has five uncoordinated writers, and mutations can be lost
silently.** `write_atomic` (`backlog_edit.rs:104`) is temp-file + rename, which
prevents a torn *read*. It does nothing about read-modify-write: two mutations
that read the same text both compute their result against it, both rename, and
the second silently erases the first. The writers today:

| Writer | When | Cite |
|---|---|---|
| the working agent (free-hand Edit) | during its iteration | `PROMPT.template.md:22,31` |
| the check-off judge (uncheck) | post-turn | `judge.rs:101-102` |
| `curate::sweep` | post-turn | `control.rs:545` |
| arc archive on completion | terminal | `control.rs:631` |
| `ralph backlog add/edit` (Discord, agents) | any time | `backlog_edit.rs:169` |

The dangerous pair is the last against the first: a `/backlog-add` landing
between the agent's read and its write is lost with no error anywhere.

**4. `loop.pid` is owned by the wrong process.** Only ralphd writes it
(`handler.rs:47`); `ralph` has no pidfile and no startup guard — verified absent
from its source. So the single-loop-per-repo invariant only holds for loops
ralphd launched. A hand-launched loop is invisible to it, and ralphd will
cheerfully spawn a second loop on the same repo: two agents, one worktree, one
branch.

## Goal

Make `ralph` alone sufficient to drive a loop end-to-end, and make the loop's own
state honest regardless of who launched it. Every `ralphd` command becomes a thin
shell-out to a subcommand that a human or an agent can run directly.

**The governing constraint: `ralphd` must never be load-bearing.** It holds no
state that does not already live in `.ralph/` or its own config. This is testable
— a suite drives a loop start to finish with no `ralphd` process anywhere.

Non-goals: multi-loop `ralphd` (a separate plan — this spec is its prerequisite);
fleet concurrency capping (dropped: `ralph` already backs off on LIMIT, and it was
the main thing pulling toward a privileged daemon); replacing the `.ralph/` file
protocol with a socket (see Rejected alternatives).

## Design

### 1. `ralph` owns `loop.pid`, guarded by `create_new`

At startup the supervisor creates `<dir>/loop.pid` with
`OpenOptions::new().write(true).create_new(true)` — atomic at the syscall level,
no lock primitive, no crate, no `unsafe`. On `AlreadyExists`: read the pid and
probe it with `kill(pid, 0)`; if it is alive, exit non-zero with
`a loop is already running in this repo (pid N)`; if it is dead, unlink the stale
file and retry once, then give up.

The liveness half already exists as ralphd's `loop_pid.rs:13-48`, including its
guards against pid 0 and oversized pids (which would wrap to a negative
process-group target). Move that module into `ralph`; ralphd keeps *reading* the
pidfile but stops writing it, and `handler.rs:launch_and_record` drops its
`loop_pid::write` call.

The file is removed on graceful shutdown. A `SIGKILL`ed loop leaves it stale,
which the liveness probe handles — that is why the probe is not optional.

**Not flock.** See Rejected alternatives.

### 2. Backlog mutations: single writer, inbox queue

Exactly one process writes `BACKLOG.md` at a time, and **no writer ever mutates
it in place from the CLI**. The two halves are separated:

**Enqueue (always).** `ralph add|drop|uncheck` never touches `BACKLOG.md`. It
writes a request file to `<dir>/inbox/<unix_ts>-<pid>-<counter>.json`. Unique
filenames mean there is nothing to arbitrate — every writer creates its own file,
nobody contends, no lock is needed.

**Drain (whoever holds the guard).** Applying the queue is guarded by a
short-lived `<dir>/drain.pid` taken with the same `create_new` + liveness probe
as §1. Two drainers cannot overlap, and a drainer killed mid-run leaves a stale
file the next probe clears.

- **A loop is running** — it drains at the iteration boundary (below). The CLI
  returns `queued (applies at the next iteration boundary)`.
- **No loop running** — the CLI takes the drain guard itself immediately after
  enqueueing and applies the queue synchronously, so interactive terminal use
  stays instant and single-step.

Separating enqueue from drain is what removes the check-then-write window: the
CLI's decision is only about *who drains*, never about whether it is safe to
write. A loop that starts a microsecond later simply drains a queue that already
has the request in it, and two concurrent CLI invocations with no loop running
resolve to "one of them drained both requests", which is correct rather than
lossy. Draining is idempotent over the queue.

The loop **drains the inbox at the top of each iteration**, before
`context::load` (`control.rs:312`) resolves the next leaf — so queued work is
visible to routing, and the drain runs on every iteration regardless of whether
the previous turn succeeded. Requests apply in filename order, which is
timestamp order.

Order over one cycle: **drain → route → run turn → judge → sweep.**

This closes the agent-vs-external race by construction rather than by
cooperation: the agent holds `BACKLOG.md` exclusively for the duration of its
iteration, and it never has to take a lock it cannot be made to take.

**Deferred failure.** A queued mutation that turns out to be invalid at drain
time is rejected then, not at call time. Mitigate two ways: lint the request
against the *current* backlog at enqueue and refuse immediately (this catches
essentially everything — duplicate id, placeholder `Verify:`, missing parent);
and report any drain-time rejection to `run.log` and the webhook, with the
request file moved to `<dir>/inbox/rejected/` rather than deleted.

**Cost to accept:** an agent that runs `ralph add` mid-iteration will not see its
own addition in a following `ralph lint` during that same iteration. Note it in
`PROMPT.template.md`; it is the correct behavior (the backlog must not shift
under a running agent) but it is surprising.

### 3. `add`, `drop`, `done`, `uncheck`

```
ralph add [<id>] <title> [--verify <cmd>]   # body from stdin when piped
ralph add --under <parent> <title>          # auto-numbers <parent>.N
ralph drop <id> [--recursive]
ralph done <id>
ralph uncheck <id>
```

All three route through §2 and keep the existing **lint-or-reject** contract: the
result is parsed in memory and, if it has errors, never reaches disk
(`backlog_edit.rs:53-57`).

**`add`**
- No `<id>` → today's behavior, next top-level id via `next_top_level_id`.
- Explicit `<id>` → insert as the last child of the parent implied by the id
  (`3.1.1` → under `3.1`), at the parent's indent + 2, per the schema
  (`BACKLOG.schema.md:28`). The parent must exist; error naming it if not.
- **Conflict is an error.** A duplicate id is already a lint error
  (`backlog.rs:140`), so the reject gate catches it — but it surfaces as a lint
  dump. Pre-check explicitly and fail with `id 3.1.1 already exists (line 47)`
  and a distinct exit code.
- `--under <parent>` assigns the next free `<parent>.N`, so the caller does not
  have to know the numbering.
- Body: `--verify <cmd>` for the one-line case (Discord, quick terminal use); if
  stdin is not a TTY, read the full body (prose + `Verify:` line) from it, which
  is the ergonomic path for an agent using a heredoc.

**`drop`** — removes a task's own body *and* its subtree.
- Unknown id → error, mirroring `apply_edit`'s at `backlog_edit.rs:134`.
- Has children → refuse unless `--recursive`. A silent cascade is how work
  disappears.
- Is the currently selected leaf *and* a loop is live → refuse. Detectable only
  because §1 gives `ralph` an honest pidfile.
- The removed subtree is appended to `<dir>/archive/dropped-<timestamp>.md`, so
  `drop` is never destructive. This matches the codebase's existing temperament:
  lint-or-reject, atomic writes, archive-on-completion.

**`uncheck`** — `apply_uncheck` already exists (`backlog_edit.rs:64`) and is used
by the judge (`judge.rs:101`), but is unreachable from the CLI: `run()` matches
only `add` and `edit` (`:185`). Expose it. It is the natural companion to `add`
("reopen `3.1` so I can queue a stage under it"), and it is free.

**`done`** — flips `[ ]` to `[x]` on one id; the inverse of `uncheck` and
structurally its mirror. It does *not* cascade: per the schema
(`BACKLOG.schema.md:31-35`) a parent with pending children is a container that
becomes its own integration step, so closing the last child must not close the
parent. Checking a parent that still has unchecked descendants is caught by the
existing "checked parent contains an unchecked stage" lint and rejected, which is
the correct answer rather than a special case.

`done` also lets the **working agent** stop hand-editing `BACKLOG.md`. Update
`PROMPT.template.md:22` to say stages are added with `ralph add`, and `:31` to
say a finished task is closed with `ralph done <id>`. This is not required for
§2's correctness — the agent already owns the file exclusively during its
iteration — but it means every mutation in the system flows through one
schema-checked, lint-or-reject path, so a malformed check-off becomes impossible
rather than merely unlikely. It is also what makes the `msg` preamble in §6 true
without qualification: *never edit `BACKLOG.md`, use the CLI*.

`ralph backlog add|edit` remain as aliases so ralphd (`ralph.rs:54,58`) keeps
working until it is updated.

### 4. `ralph model <tier>`

Validate against the configured ladder and write the one-shot `.ralph/MODEL`
override that `State::take_model` already consumes (`state.rs:75`). This is a
direct lift of ralphd's `model.rs:validate_tier` plus `ralph.rs:65`, moved to the
side of the fence that owns the file format. ralphd's `/model` then shells out
like every other command.

Unlike backlog mutations this needs no queue: `MODEL` is single-valued,
last-write-wins is the correct semantic, and `take_model` clears it on read.

### 5. `ralph stop --now`

`ralph stop` writes `STOP` (`state.rs:192`), honored only at an iteration
boundary — so a hung turn can ignore it for the full `--iteration-timeout`, or
indefinitely when that is `0`.

`--now` additionally sends `SIGTERM` to the pid in §1's file. Three details make
this correct, and the first two are easy to get wrong:

- **The recorded pid is the supervisor, not the loop.** `supervisor::run` forks a
  child that runs `control::run` and only `waitpid`s on it, so a `SIGTERM`
  delivered to the recorded pid stops the watcher and leaves the loop running.
  The supervisor therefore installs a `SIGTERM` handler that **forwards the
  signal to its child** and then continues into its normal `waitpid` path, so the
  death is interpreted by the existing machinery.
- **Signal the pid, never the process group.** `kill(-pid, …)` is correct only
  when the supervisor leads its own group — true when ralphd launched it
  (`ralph.rs:87-96` calls `setsid`), false for a hand-launched `ralph`, which
  sits in the invoking shell's group and would take the shell down with it.
  Forwarding pid-to-pid avoids depending on how the loop was started.
- **The loop's handler tears down `claude` through `kill_group`.** `claude` is
  spawned into its **own session** (`control.rs:719-732`), so it is unreachable
  from any signal aimed at ralph. The child's handler calls the existing
  `kill_group` (`control.rs:970`) on the current `claude` pid — an `AtomicU32`
  set in `run_one` — then exits. `libc::kill` is async-signal-safe, so calling it
  from a handler is legal. Skipping this orphans an expensive `claude` tree.

`SIGTERM` is already classified as deliberate termination (`supervisor.rs:75-85`),
so `--restart` is correctly suppressed and the exit follows the `128 + signal`
convention.

`STOP` is written *as well as* the signal, so a loop that is between iterations
halts gracefully rather than being killed.

### 6. `ralph msg` — a persistent steering session

Replaces ralphd's `/btw`, which is stateless: every message re-establishes
context, so "no, do it the other way" does not work.

```
ralph msg <text>     # resume this loop's session, or create it
ralph msg --new      # start a fresh session, archiving the old id
```

Mechanism (flags verified against the installed `claude`):

- First call generates a UUID (read `/proc/sys/kernel/random/uuid` — no new
  crate; `ralph` is deliberately down to `libc`/`serde`/`serde_json`/`toml`),
  stores it in `<dir>/msg-session`, and runs
  `claude -p --output-format stream-json --verbose --session-id <uuid>`.
- Later calls run the same with `--resume <uuid>`.
- `--append-system-prompt` carries the steering preamble.

**The preamble is short, because the CLI is the control surface.** A session with
a shell already has full loop control — no tool definitions, no MCP server, no
API. It says: you are attached to the ralph loop in this repo; `ralph status`
shows the frontier; `.ralph/live` is the running iteration; `run.log` is history;
**queue work with `ralph add`, never by editing `BACKLOG.md`**; `ralph model`
retiers the next pass; `ralph stop` halts after the current task.

That last instruction is enforced by §2 rather than trusted: a session that edits
`BACKLOG.md` by hand while a loop runs will have its edit overwritten, and one
that uses `ralph add` is queued safely. **`msg` steers the loop; it does not do
the work.** This keeps sessions cheap, which matters for phone use.

Two guards:

- **Concurrent invocations.** Two messages before the first returns would run two
  `--resume` against one session id. Reuse §1's `create_new` pattern on
  `<dir>/msg.pid`; a second concurrent `msg` is refused, not queued.
- **Unbounded growth.** A persistent session accretes context until it is
  expensive and then until it does not fit. `--new` resets it, and the arc
  completion path (`control.rs:631`, which already archives `BACKLOG.md`)
  archives the session id too — a natural boundary.

ralphd's `/msg` shells out. Its `btw.rs` streaming machinery is kept unchanged:
the live-message-edit cadence and, critically, the migration off the interaction
token before Discord's 15-minute expiry (`btw.rs:28`).

Unlike `/btw`, `msg` **earns** its CLI mirror: the session id is state in
`.ralph/`, so the conversation is a loop artifact either front-end can attach to
— start it from your phone, continue it from the terminal on the same context.

### 7. Per-loop budget ledger

`--max-cost` is checked at `control.rs:282` against a `cost_total` accumulated in
memory over one run, so it resets whenever the loop restarts — including a
`--restart` relaunch. A budget that survives that needs persistence.

Append one line per iteration to `<dir>/ledger.jsonl`:
`{"ts":<unix>,"iter":N,"model":"sonnet","cost_usd":0.42}`. `O_APPEND` writes
below `PIPE_BUF` are atomic, and there is exactly one writer anyway.

Add `budget_usd` and `budget_window` (a `DurationSpec`, so `"24h"` works) to
`FileConfig` (`config.rs:102`). At the same check as `max_cost_usd`, sum the
ledger tail over the window and halt when it is exceeded, with the existing
budget-halt webhook path.

**Enforcement lives in `ralph`, not `ralphd`** — anything gated only at ralphd's
spawn is bypassed by running `ralph` directly, which is normal practice here.
ralphd reads the ledger to render spend on the status card and to warn at 80%.

Non-finite guard: `cost_usd` comes from the result envelope and is used in
arithmetic; write `0.0` rather than a `NaN`/`inf` that would make the line
unparseable.

## Rejected alternatives

**`flock` for either invariant.** Delicate in general and specifically wrong
here. The primary repo lives on `/mnt/c` — drvfs/9p under WSL2 — where advisory
lock semantics are exactly the class of thing that passes a test and fails
overnight. More fundamentally, a lock only serializes writers that take it, and
the working agent edits `BACKLOG.md` with its Edit tool (`PROMPT.template.md:31`)
and will never take one. So flock costs real fragility at the exact place it
protects least. `create_new` (§1) and single-writer-by-construction (§2) need no
lock primitive at all.

**A control socket on `ralph`.** Considered for immediate stop and structured
status. `ralph` is a deliberately synchronous, forking runner with no async
runtime; a listener would have to survive the supervisor's `fork` and the
between-iteration group kills. Everything a socket would carry is already in
`.ralph/`, except immediate stop — which is a signal (§5), not a protocol.

**A control socket on `ralphd`, with a `ralphctl` client.** Rejected because it
inverts the dependency we want. The `.ralph/` file protocol is inherently
multi-client: any local agent drives a loop today with no coordination. Routing
control through a daemon makes `ralphd` privileged and load-bearing, which is the
opposite of this spec's governing constraint. `ralphctl` is just `ralph`.

**Feature-gating Discord out of `ralphd`.** Moot once ralphd is not load-bearing:
strip Discord from it and what remains is a config file and a status-card
renderer, and the card is a Discord artifact. In a Discord-hostile environment
you do not run `ralphd` — you run `ralph`.

**Mirroring `/btw` in the CLI.** `/btw` exists because you cannot open a terminal
from a phone. On a box where Discord is a security hole you have a terminal, and
`/btw` is `claude -p`. Mirroring it would be building a worse terminal. `msg`
replaces it and is mirrored for a different reason — it holds state.

**An append-only journal with `BACKLOG.md` derived from it.** A clean answer to
the multi-writer problem, but `BACKLOG.md` is authored and read by humans and
agents. Making it a generated artifact breaks the thing that makes the schema
work.

**Fleet concurrency capping.** Dropped. `ralph` already classifies and backs off
on LIMIT, the realistic fleet is a handful of loops, and honest enforcement would
need either a privileged daemon or a shared lock directory — both of which cut
against this spec.
