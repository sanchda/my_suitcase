# ralphd — Discord control bridge for the ralph loop

**Date:** 2026-07-21
**Status:** Approved design

## Summary

`ralphd` is a new, always-on foreground binary that lets a single authorized
Discord user drive a `ralph` autonomous loop from a single Discord channel via
native slash commands. It is a thin **Discord-to-CLI bridge**: it never
reimplements loop logic, only translates slash commands into the same `.ralph/`
file writes and `ralph` CLI calls an operator would issue by hand.

The existing `ralph` binary stays synchronous and fork-based and is untouched
except for a few **additive** subcommands that keep it the single owner of
backlog logic.

## Goals

- Control a `ralph` loop from Discord: start, stop, one-shot model override.
- Query the loop: backlog stats, current task, upcoming tasks.
- Curate the backlog: add and edit tasks, safely, without ever crashing a
  running loop.
- Strict single-tenant authorization: exactly one guild, one channel, one user.

## Non-goals (v1)

- Multi-repo / multi-channel / multi-user operation (explicitly single-tenant).
- Force-kill of a running loop (`/stop` is graceful only).
- ralphd relaying loop progress into the channel — `ralph`'s existing
  `DISCORD_WEBHOOK` keeps posting lifecycle/progress events (start, escalate,
  complete, abort, per-iteration). ralphd only handles inbound commands and
  their direct replies.
- Ad-hoc per-`/start` config overrides — the loop profile is fixed at ralphd
  launch via arg-forwarding.

## Architecture

Two binaries, clean split of responsibility:

- **`ralph`** (existing, `tools/ralph/`) — the loop. Deliberately synchronous
  and fork-based (see `supervisor.rs`). Gains additive subcommands only:
  `ralph status --json`, `ralph backlog add`, `ralph backlog edit`.
- **`ralphd`** (new, **its own crate** `tools/ralphd/`) — the always-on
  foreground Discord listener. Async (`serenity` + `tokio`). It shells out to
  the `ralph` binary on PATH and reads/writes `.ralph/` files. It links none of
  ralph's code, so ralph's lean synchronous build never pulls in tokio/serenity.

**Control plane = the `.ralph/` files + the `ralph` CLI, unchanged.** This is
what keeps ralphd thin and keeps ralph free of any Discord awareness.

Rejected alternative: a single crate with a `bot` cargo feature gating the async
deps. Works, but couples the two build graphs and risks async deps leaking into
ralph's build. A separate crate that shells out is simpler and keeps ralph
untouched.

## ralphd configuration & launch

Launched in the repo root, foreground (use `nohup`/systemd for persistence):

```
DISCORD_BOT_TOKEN=... ralphd \
  --guild <GUILD_ID> --channel <CHANNEL_ID> --user <USER_ID> \
  [--working-dir <path>] \
  -- --model sonnet --max-cost 25 --max-iterations 50
```

- ralphd's own settings come from flags **before** `--` (or env fallbacks:
  `RALPHD_GUILD_ID`, `RALPHD_CHANNEL_ID`, `RALPHD_USER_ID`,
  `RALPHD_WORKING_DIR`).
- `DISCORD_BOT_TOKEN` is secret — **env only**, never a flag.
- Everything **after `--`** is the fixed loop profile, forwarded verbatim to
  `ralph` on `/start`. `--working-dir` defaults to the current directory (like
  `ralph`).
- The bot must be invited with `bot` + `applications.commands` scopes only.

## Authorization (single-tenant)

Every incoming interaction is rejected with an **ephemeral** error unless BOTH:

- `interaction.channel_id == <CHANNEL_ID>`, and
- `interaction.user.id == <USER_ID>`.

Slash commands are registered **guild-scoped** to the one configured guild
(instant registration, not visible in any other guild). No global commands.

This is the whole security model: a Discord message can steer an autonomous
`--dangerously-skip-permissions` agent, so no unauthorized channel or user may
reach any command.

## Commands (v1)

Native slash commands with typed arguments (no text parsing).

| Command | Mechanism | Reply |
|---|---|---|
| `/start` | If `.ralph/loop.pid` is live → refuse. Else spawn `ralph <forwarded launch args>` in the working dir, write the pid. | `started (iter N)` or `already running (iter N)` |
| `/stop` | `ralph stop` (writes `.ralph/STOP`; graceful halt after current task) | confirmation |
| `/model <tier>` | Validate `tier ∈ {haiku,sonnet,opus}`; write `.ralph/MODEL` | `next iteration → <tier> (one-shot)` |
| `/status` | `ralph status --json` + ralphd's own pid-liveness → embed | running?, iteration, pending count, current task, next few |
| `/next` | Same JSON; shows the current selected task + upcoming N labels | task list |
| `/backlog add <text>` | `ralph backlog add "<text>"` | new task ID, or the lint error |
| `/backlog edit <id> <text>` | `ralph backlog edit <id> "<text>"` | confirmation, or lint error |

`/model` is a **one-shot** override for the next iteration only (matches
`.ralph/MODEL` semantics in `state.rs`); the reply says so. It is not a permanent
default — that lives in the task's `(tier/…)` decoration or `ralph.toml`.

## New `ralph` subcommands

All additive; backed by the existing `backlog.rs` / `context.rs` code.

### `ralph status --json`

Emits a single JSON object of repo-derived facts:

```json
{
  "iteration": 12,
  "pending_leaf_count": 34,
  "current": { "label": "…selected leaf label…", "excerpt": "…own excerpt…" },
  "upcoming": ["next leaf label", "…", "…"]
}
```

Sourced from `context::load(&backlog, &progress)` and `Document`
(`pending_leaf_count`, `selected_index`, `selected_path`, `own_excerpt`,
`upcoming_leaf_labels`). Run-state (is a loop alive?) is **not** ralph's to
know — ralphd merges that in from its own `.ralph/loop.pid`.

### `ralph backlog add "<text>"`

Appends a well-formed top-level task to `BACKLOG.md`: an auto-generated unique ID
and a `Verify:` placeholder, formatted per the v1 schema. Then runs the lint
path. See the safety gate below.

### `ralph backlog edit <id> "<text>"`

Locates the task by ID via the parser, replaces its own line span (the leaf's
own excerpt / label), then runs the lint path. v1 scope is single-leaf text
replacement — not structural reshaping (no re-parenting, reordering, or stage
restructuring).

## The safety gate (protects a running loop)

In `control.rs` (`context::load` … `resolved.has_errors()`), a running loop
**aborts** if it reads an invalid backlog. Therefore every backlog mutation is:

1. Write the proposed new content to a temp file.
2. Lint it (the same validation `ralph lint` uses).
3. On success: **atomic rename** over `BACKLOG.md` (so the loop never reads a
   half-written file).
4. On failure: discard the temp file, leave `BACKLOG.md` untouched, exit
   non-zero with the lint error (surfaced to Discord).

A bad `/backlog add|edit` therefore never reaches disk and cannot crash a live
loop. Valid edits land on the loop's next iteration by design — that is the
intended mechanism for steering a running loop.

## Lifecycle & concurrency

- **Single loop** enforced via `.ralph/loop.pid`: ralphd writes it on spawn and
  checks liveness with `kill(pid, 0)`. `/start` refuses while a live pid exists.
- **ralphd restart**: on startup ralphd adopts an existing live pid (so a
  restart doesn't lose track of a running loop) and clears a stale one.
- `/stop` is graceful-only in v1 (writes STOP via `ralph stop`); no force-kill.
- Backlog edits during a run take effect on the next iteration — intended.

## Error handling

- Every `ralph` invocation's non-zero exit and stderr is surfaced to the user as
  a Discord reply (ralphd does not silently swallow failures the way the
  fire-and-forget webhook does).
- Auth failures → ephemeral reply, no action taken, logged locally.
- Discord gateway disconnects → serenity's built-in reconnect; ralphd re-adopts
  loop state from `.ralph/` on reconnect (state is all on disk).

## Build & install

- `ralphd` is a separate crate under `tools/ralphd/`.
- `personalize/scripts/setup_ralph.sh` builds and installs **both** `ralph` and
  `ralphd` to `~/.local/bin/`.
- ralph's build graph is unchanged (no new deps).

## Testing

- **ralph primitives** (unit): `backlog add`/`edit` round-trip; lint-revert
  leaves `BACKLOG.md` untouched on invalid input; atomic-rename never leaves a
  partial file; `status --json` shape.
- **ralphd** (unit): the auth gate (channel + user predicate) is pure and
  directly testable; command→action mapping is testable with a mocked CLI
  runner; pid liveness/adoption logic is unit-testable.
- **Gateway**: live Discord connection is manual/integration only.
