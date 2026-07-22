# ralph: supervisor-owned restart, `ralph stop`, and per-iteration progress webhooks

Date: 2026-07-21
Status: approved (user delegated implementation with "make your best judgments")

## Motivation

Three related lifecycle improvements to the ralph runner:

1. **Restart on ungraceful death.** Today a double-forked, detached watchdog
   (`watchdog.rs`) polls `/proc/<pid>` and *reports* an ungraceful death (OOM,
   kill, crash) but cannot do anything about it. We want ralph to optionally
   **restart** itself after such a death. Because the loop's state lives entirely
   in `.ralph/` files (iteration counter, PROGRESS, BACKLOG), a fresh process
   resumes cleanly — no in-memory state to preserve.

2. **`ralph stop` subcommand.** Today you halt the loop by manually
   `touch .ralph/STOP`. Add a first-class `ralph stop` that writes the same
   marker, halting the loop after it finishes the current task — and, critically,
   suppressing an otherwise-pending restart.

3. **Per-iteration progress webhook.** On each successful return to the loop,
   post a Discord line with a point-in-time estimate of remaining work
   (`iter N/M`) plus the one-line iteration summary ralph already logs. Webhook
   messages are free, so surfacing progress costs nothing.

## Design

### 1. Supervisor replaces the detached watchdog

`watchdog.rs` is **deleted**. The top-level `ralph` process becomes a
**non-detached supervisor (parent)** that owns the child loop:

```
main() → parse config → supervisor::run(&cfg)
supervisor::run:
  loop {
    flush stdout
    fork()
      child:  exit(control::run(&cfg))      // the actual loop; never returns here
      parent: waitpid(child)                // measure child lifetime with Instant
        WIFEXITED(code) → return code       // graceful/panic: terminal, no restart
        WIFSIGNALED(sig):                   // ungraceful death (SIGKILL/OOM/SIGSEGV)
          post "💀 ralph terminated" (webhook)   // the watchdog's old job
          if cfg.restart && !STOP present:
            crash-loop guard + backoff sleep → continue (fork again)
          else:
            clear STOP if present → return 1
  }
```

Fork (not exec) is safe and sufficient: the parent is single-threaded (it only
ever `waitpid`s), so every fork happens from a single-threaded process; the child
re-runs `control::run`, which re-opens `State` from `.ralph/` and resumes at the
persisted iteration. `control::run`'s own start notification (`🟢 ralph started
… from iter N`) naturally announces the resumed iteration on each relaunch.

**When we fork.** The supervisor forks only when there is something to supervise:
`cfg.restart || webhook configured`. Otherwise it runs `control::run` inline
(zero fork overhead for the plain local case). This preserves the old behavior
that a configured webhook gets death notifications, and adds restart on top.

**Restart trigger scope.** Only `WIFSIGNALED` (killed by a signal) restarts —
the "basically just SIGKILL/OOM" case — and only for crash/kill signals.
Deliberate-termination signals (`SIGINT`, `SIGTERM`, `SIGHUP`, `SIGQUIT`) are
excluded, so a targeted `kill`/Ctrl-C always stops the loop even with `--restart`
on. `WIFEXITED` covers every graceful exit *and* a Rust panic (exit 101, since
the crate unwinds rather than aborts), so a deterministic panic-bug does not
spin. Ctrl-C also sends SIGINT to the whole foreground group, terminating the
parent too, so it stops everything.

**Crash-loop guard.** A child that dies by signal in under `MIN_HEALTHY_SECS`
(60s) increments a rapid-failure counter; after `MAX_RAPID_RESTARTS` (5)
consecutive rapid failures the supervisor gives up and returns non-zero. A child
that lived longer resets the counter. Each restart waits `RESTART_BACKOFF_SECS`
(10s) first. Constants live in `supervisor.rs`.

**STOP suppresses restart.** Before restarting, the parent checks
`State::stop_requested()`. If STOP is present (e.g. `ralph stop` raced the kill),
it does not restart and clears STOP — the stop request is honored and consumed.

### Tradeoff (accepted)

The old watchdog was reparented to init and survived even a session-wide kill; a
non-detached parent dies with the session (terminal close, group-wide OOM), so
nothing reports in that case. It *does* catch the common case — the OOM killer or
a hang-kill targeting the memory-heavy child loop while the tiny idle parent
survives. The user chose this simpler model explicitly.

### 2. `ralph stop`

New subcommand handled in `main::run`, alongside `init`/`schema`/`brief`/`lint`:
resolve config (so `--dir`/`--config`/`RALPH_DIR` are honored), then write the
STOP marker via a new `State::request_stop()` and print a confirmation. The loop
already checks `stop_requested()` at the top of each iteration and halts
gracefully after the current task; the supervisor change above makes STOP also
suppress restart.

### 3. Per-iteration progress webhook

In `control.rs`, at the end of a **successful** iteration (after the synth +
curate hand-off block — the "return to the loop" boundary), post one webhook
line combining progress and the summary ralph already logs:

- **Denominator.** If `max_iterations > 0`: `iter N/max_iterations`. Otherwise
  (unlimited): `iter N (~M tasks pending)`, where `M` is the count of pending
  executable backlog leaves — a new `backlog::Document::pending_leaf_count()`
  (tasks that are unchecked and have no unchecked descendant, the same predicate
  `selected_index` uses). Point-in-time; it shifts as the backlog is curated,
  which is acceptable.
- **Summary.** The existing one-line `ok ($cost) — <first-160-chars>` snippet.

Example posts:
- `🔄 **iter 12/200** — ok ($0.0431) — Added pending_leaf_count and wired …`
- `🔄 **iter 12** (~35 pending) — ok ($0.0431) — Added pending_leaf_count …`

Only successful iterations post (real progress). Rate-limit waits and transient
retries keep their existing log lines and do not spam the webhook.

## Config surface

New field `restart: bool` (default `false`) across the precedence chain:
- flag: `--restart <true|false>` (valued, matching the user's `-restart true`)
- env: `RALPH_RESTART` (truthy unless `0`/`false`, matching `RALPH_YOLO`)
- file: `restart = true` in `ralph.toml`

`USAGE` gains the `--restart` line, the `ralph stop` subcommand line, and a note
that restart only fires on ungraceful death and is suppressed by STOP.

## Testing

- `supervisor.rs`: pure unit tests for the crash-loop guard state machine
  (rapid-failure counting, reset on healthy lifetime, give-up threshold) and for
  the wait-status interpretation helper (exited vs signalled → restart decision),
  factored so they don't actually fork.
- `backlog.rs`: `pending_leaf_count` on representative docs (nested, all-checked,
  mixed).
- `config.rs`: `restart` precedence (default/file/env/flag) mirroring existing
  precedence tests; `--restart` value parsing (true/false/invalid).
- `state.rs`: `request_stop` writes a file `stop_requested` then sees.
- `control.rs`: progress-line formatting helper (bounded vs unlimited
  denominator) as a pure function tested in isolation.
- `tests/cli.rs`: `ralph stop` writes `.ralph/STOP`; `--help` lists `ralph stop`
  and `--restart`.

## Out of scope

- Restarting on graceful halts, COMPLETE, or ABORT (terminal by design).
- Surviving a session-wide kill (see accepted tradeoff).
- Persisting/exec-ing a serialized arg copy — fork inherits the parsed `Config`
  in memory, so no on-disk arg snapshot is needed.
