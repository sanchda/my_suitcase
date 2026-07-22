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
| Config (optional) | `.ralph/ralph.toml` | **local** |
| Runtime (counter, logs, MODEL/STATUS) | `.ralph/` (gitignored) | **local**, generated |

Rule of thumb: **the runner is global; the prompts, config, and record-keeping
are local.**

The **entire** `.ralph/` working set — config, backlog, progress, logs, archive
— is gitignored runtime state, never product. Product commits carry only code.
`ralph init` writes a single `/.ralph/` ignore for you (see the block below), so
nothing under `.ralph/` is tracked.

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
2. Flesh out `.ralph/BACKLOG.md` using the v1 schema, optionally add a VISION.
   PROGRESS is runner-owned — no need to seed it; the orchestrator writes a
   carry-forward note there after each iteration.
3. `ralph init` already wrote the `.gitignore` block for you (see below) — no
   manual step needed.
4. Check routing, then run it on a dedicated branch:

   ```bash
   ralph schema
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
# The entire loop working set is runtime state, never product — commit code only.
/.ralph/
```

This ignores the entire `.ralph/` working set — config, backlog, progress,
logs, archive alike — so product commits carry only code.

## Deterministic backlog schema and staging

Run `ralph schema` for the complete, version-matched authoring reference. In
short: tasks are ordered Markdown checkboxes with unique IDs and `Verify:`
contracts; two-space-indented children are explicit stages. `ralph lint`
validates and selects the next leaf, while `ralph brief` shows the bounded
context the model will receive. The same reference lives in
[BACKLOG.schema.md](BACKLOG.schema.md).

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

## Query & edit from the CLI
- `ralph status [--json]` — a snapshot of the backlog frontier: iteration,
  pending-leaf count, the current selected task, and the next few upcoming
  tasks. `--json` emits one machine-readable line.
- `ralph backlog add --title "<t>" --verify "<cmd>"` — append a well-formed task
  (auto-assigned top-level id). Rejected without touching the file if the result
  would fail schema lint.
- `ralph backlog edit --id <id> --title "<t>" --verify "<cmd>"` — replace a
  task's title and verify in place (children preserved). Same lint-or-revert
  safety. Both writes are atomic, so they never expose a half-written backlog to
  a running loop.

## ralphd — Discord control bridge
`ralphd` is a separate, always-on foreground binary that lets one authorized
Discord user drive the loop from one channel via native slash commands. It shells
out to `ralph` and reads/writes `.ralph/`; it is single-tenant by design (one
guild, one channel, one user id — every other channel/user is refused).

```bash
DISCORD_BOT_TOKEN=… ralphd \
  --guild <GUILD_ID> --channel <CHANNEL_ID> --user <USER_ID> \
  [--working-dir <repo>] -- <ralph args forwarded to /start>
```

Run `ralphd --help` (or `ralphd help`) for the full usage. Every setting below
takes a flag **or** an environment variable (flag wins); the token is env-only.
Everything after `--` is forwarded verbatim to `ralph` on `/start`.

| Setting | Flag | Env |
|---------|------|-----|
| Bot token | — (env only) | `DISCORD_BOT_TOKEN` |
| Guild (server) id | `--guild <id>` | `RALPHD_GUILD_ID` |
| Channel id | `--channel <id>` | `RALPHD_CHANNEL_ID` |
| Authorized user id | `--user <id>` | `RALPHD_USER_ID` |
| Working dir (repo, default `.`) | `--working-dir <path>` | `RALPHD_WORKING_DIR` |

Commands: `/start [model]`, `/stop`, `/model <tier>`, `/status`, `/next`,
`/backlog-add`, `/backlog-edit`, `/btw <message> [model]`. `/start` takes an
optional model to override the launch default for one run; `/btw` runs a one-off
yolo `claude` session with your message (optionally on a given model) and posts
its output back. Loop lifecycle/progress still posts via
`ralph`'s existing `DISCORD_WEBHOOK` (point it at the same channel); ralphd only
handles inbound commands and their replies. Invite the bot with the `bot` +
`applications.commands` scopes.

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

- `.ralph/MODEL` — `haiku` / `sonnet` / `opus`, a **one-shot override** sizing
  the NEXT iteration; cleared once read. Normally unset: a task's own `(tier/…)`
  decoration is the baseline (see below), and `MODEL` only overrides it for a
  single pass.
- `.ralph/STATUS` — this iteration's type: `code` (a normal committing
  iteration), or `review`/`plan`/`blocked` for an intentional non-code pass.
  Absent is treated as `code`.

Invalid `MODEL` values are ignored with a warning (never abort). See the PROMPT
template for the exact instructions given to the model.

**Model precedence** (highest first): escalation override → one-shot `.ralph/MODEL`
→ the resolved leaf's own `(tier/…)` decoration (e.g. `(opus/pedagogy.)` → `opus`;
first token of the trailing tag, `haiku`/`sonnet`/`opus` only) → the run default.
So model tier lives with the task in the backlog; the agent need not restate it.

Before every process launch the runner parses the complete backlog, selects the
next leaf by document order, and appends a bounded brief containing that
leaf plus PROGRESS's carry-forward note injected verbatim — no `Next:`
parsing, no id matching. The base prompt remains first and stable for caching.
Ralph also passes `--no-session-persistence` (iterations are deliberately
fresh) and `--exclude-dynamic-system-prompt-sections` (better prompt-cache
reuse).

## No-progress detection & escalation
A **progress streak** counts consecutive unproductive iterations. An iteration
is **no-progress** when it is a `code` iteration that made no new commit, or it
was a transient/timeout retry. A declared productive non-`code` pass
(`review`/`plan`) is **excluded** and logged as such. On the streak reaching:

- `--escalate-after` (default 2): the model escalates one tier up the ladder
  `haiku → sonnet → opus` for the next attempt;
- `--abort-after` (default 4): the loop aborts with a clear reason.

A productive `code` iteration resets the streak.

A `blocked` pass is different: it means the agent has declared a dead-end only a
human can clear (a stop gate, missing authority, unresolvable ambiguity). It does
**not** escalate — a fresh identical iteration would just re-block — and the loop
**aborts after 2 consecutive** `blocked` passes rather than spinning. (Needing a
bigger model is not `blocked`; that's what the tier decoration / `MODEL` are for.)

## Budgets
Checked at iteration boundaries; each halts the loop when hit:

| Budget | Flag / env | Default |
|--------|-----------|---------|
| Cumulative cost | `--max-cost` / `RALPH_MAX_COST` | 0 (off) |
| Wall-clock | `--max-duration` / `RALPH_MAX_DURATION` (`8h`/`30m`/`300s`) | 0 (off) |
| Iterations | `--max-iterations` / `RALPH_MAX_ITER` | 0 (off) |

## Discord notifications
Set `DISCORD_WEBHOOK` to a Discord **webhook URL** (from a channel's
*Integrations → Webhooks* — it already targets that channel, so no channel id is
needed) and the loop posts lifecycle events to it: start, model escalation,
abort (no-progress **or** a hard `blocked` gate — the "come look" signal),
completion, and any budget/STOP halt. Unset = disabled. Posts go out via `curl`
with a 10s timeout and all errors swallowed, so a slow or down webhook never
stalls or fails the loop. Per-iteration results are **not** posted (they'd be
noisy); watch `.ralph/current.log` for that.

A `SIGKILL` (e.g. the OOM killer) gives ralph no chance to post its own outcome,
so at startup it also double-forks a tiny detached **watchdog** (only when a
webhook is set). The watchdog polls ralph and, if ralph vanishes without a
graceful shutdown, posts `💀 ralph terminated …` — so an overnight OOM/crash
reaches you instead of just leaving a dead terminal. A clean exit stands the
watchdog down (via a sentinel file removed on ralph's normal shutdown), so it
only ever fires for a genuine kill/crash.

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

For a fully self-contained loop, opt into a lean Claude process with
`extra_args = ["--safe-mode", "--tools", "Bash,Edit,Read,Write"]`. Safe mode
omits project instructions, hooks, plugins, MCP servers, skills, and auto-memory,
so use it only when PROMPT carries every required project/verification rule.

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
