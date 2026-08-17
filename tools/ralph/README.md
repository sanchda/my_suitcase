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
- **Stop gracefully** after the current iteration: `ralph stop` (or
  `touch .ralph/STOP`). Honored at the boundary, so a long turn finishes first.
- **Stop now**, without waiting for the turn: `ralph stop --now`. Writes STOP
  *and* signals the loop, which tears down the `claude` session group — that
  teardown is the point, since `claude` runs in its own session and a naive kill
  would orphan it and its subprocesses. Suppresses `--restart` either way.
- **Resume** later: just re-run `ralph` — the counter in `.ralph/iteration`
  persists.
- **One loop per repo is enforced.** The loop holds `.ralph/loop.pid` and a
  second `ralph` in the same repo exits 2 naming the live pid. A pidfile left by
  a killed loop is reclaimed automatically on the next start.
- **Launch detached** for overnight runs: `nohup setsid ralph … &`.

Each completed result adds a `perf` line to `run.log` with total, API, and
non-API time, turn count, and token/cache totals. This makes model time versus
local tools/tests visible without mining raw NDJSON.

## Query & edit from the CLI
- `ralph status [--json]` — a snapshot of the backlog frontier: iteration,
  pending-leaf count, the current selected task, and the next few upcoming
  tasks. `--json` emits one machine-readable line.
- `ralph add [<id>] "<title>" [--verify "<cmd>"]` — queue a task. With no id it
  takes the next top-level number; an explicit id places a child (`3.1.1` goes
  under `3.1`, whose parent must exist), and `--under <parent>` picks the next
  free `<parent>.N` for you. A duplicate id is an error, not a lint dump. Pipe
  stdin instead of `--verify` to supply a full multi-line body. When the backlog
  file is absent (a completed arc archived it away), `add` bootstraps a fresh
  schema-valid file first, so the next arc starts from `add` alone.
- `ralph done <id>` / `ralph uncheck <id>` — check off or reopen. Neither
  cascades: a parent with pending children is a container that closes as its own
  integration step, and checking one that still has unchecked descendants is
  rejected by lint.
- `ralph drop <id> [--recursive]` — remove a task. Refuses a subtree without
  `--recursive`, refuses the selected leaf while a loop runs, and appends what it
  removed to `.ralph/archive/dropped-<ts>.md` — nothing is ever deleted outright.
- `ralph model <tier>` — write the one-shot `.ralph/MODEL` override consumed by
  the next iteration. The tier must be on the configured `escalation_ladder`;
  the value is trimmed and matched case-insensitively.
- `ralph backlog add|edit …` — the older flag-style forms, kept as aliases.

Every one of these is schema-checked before it lands: the result is parsed in
memory and, if it would fail lint, rejected without touching the file.

### Mutations queue while a loop runs
No CLI command writes `BACKLOG.md` in place. Each one writes a request into
`.ralph/inbox/` under a unique filename, and the request is applied by whoever
holds the drain guard:

- **A loop is running** — it drains at the top of each iteration, before routing
  picks the next leaf. Your command prints `queued (applies at the next iteration
  boundary)` and the file does not change yet. **This is success, not failure.**
- **No loop is running** — the command drains for itself immediately, so terminal
  use stays instant and single-step.

The point is that the backlog can never shift under a running agent, and that
concurrent writers cannot lose each other's work. Before this, a `/add` landing
between the agent's read and its write vanished with no error anywhere.

A request that cannot be applied at drain time is moved to
`.ralph/inbox/rejected/` with its full replayable JSON, and reported to `run.log`
and the webhook. Enqueue-time linting catches nearly everything first.

## `ralph msg` — a persistent steering session
`ralph msg "<text>"` talks to a claude session attached to this repo's loop,
resuming the same conversation each time, so a follow-up like "no, do it the
other way" lands in context instead of re-establishing it.

```bash
ralph msg "why did 3.1 fail twice?"
ralph msg --model opus "think about whether 4 is even the right shape"
ralph msg --new                  # retire the session; next msg starts fresh
```

- **It steers; it does not do the work.** Its preamble points it at `ralph
  status`, `.ralph/live`, `run.log` and the mutation commands above, and tells it
  to queue work rather than implement it. That is what keeps a session cheap
  enough to drive from a phone.
- **`--model` is sticky** — it repins the thread until changed or `--new`, and
  the active model prints to stderr on every call so a lingering `opus` pin is
  never an invisible cost. It accepts anything `claude` accepts (`opus`, or a
  full name like `claude-fable-5`) and is *not* checked against the escalation
  ladder, which governs the loop rather than this session.
- Only one `msg` runs at a time; a second is refused, not queued.
- The session id lives in `.ralph/msg-session` and is recorded only after claude
  exits cleanly, so a failed first call cannot leave an id that every later
  resume fails against. Completing an arc retires it along with the backlog.

## ralphd — Discord control bridge
`ralphd` is a separate, always-on foreground binary that lets one authorized
Discord user drive **one loop per channel** via native slash commands. It shells
out to `ralph` for everything and only *reads* `.ralph/`, so it is never
load-bearing: anything you can do from Discord you can do from a terminal, and
killing ralphd loses nothing.

The channel a command is typed in is what selects the loop — `/status` in
`#number-grove` means that repo. There is no `--repo` argument.

```toml
# ~/.config/ralphd.toml   (or --config <path> / RALPHD_CONFIG)
guild = 123          # one guild
user  = 456          # the one authorized user

[[loop]]
name      = "number-grove"
channel   = 111
dir       = "/home/me/dev/number_grove"
args      = ["--model", "sonnet"]                    # forwarded to ralph on /start
webhook   = "https://discord.com/api/webhooks/…"     # optional, per loop
autostart = false                                    # optional
```

```bash
DISCORD_BOT_TOKEN=… ralphd --config ~/.config/ralphd.toml
```

The original single-loop flag form still works unchanged as the degenerate case:

```bash
DISCORD_BOT_TOKEN=… ralphd \
  --guild <GUILD_ID> --channel <CHANNEL_ID> --user <USER_ID> \
  [--working-dir <repo>] -- <ralph args forwarded to /start>
```

Run `ralphd --help` (or `ralphd help`) for the full usage. Every setting below
takes a flag **or** an environment variable (flag wins); the token is env-only.
An explicit `--config` always wins; otherwise the flag form wins over the
default config path, so an existing launch is never hijacked by a stale file.

| Setting | Flag | Env |
|---------|------|-----|
| Bot token | — (env only) | `DISCORD_BOT_TOKEN` |
| Config file | `--config <path>` | `RALPHD_CONFIG` |
| Guild (server) id | `--guild <id>` | `RALPHD_GUILD_ID` |
| Channel id | `--channel <id>` | `RALPHD_CHANNEL_ID` |
| Authorized user id | `--user <id>` | `RALPHD_USER_ID` |
| Working dir (repo, default `.`) | `--working-dir <path>` | `RALPHD_WORKING_DIR` |

**Run exactly one ralphd per guild.** Registering slash commands *replaces the
guild's entire command set*, so two instances in one guild silently unregister
each other's commands, last one to connect wins. ralphd logs the full list it is
about to overwrite on connect — if that list contains commands you did not
expect, another instance is running. Many loops are what the config file is for;
a second process is not.

**Each loop's `DISCORD_WEBHOOK` is set on its own child.** `ralph` reads the
webhook from the environment only, so an inherited one would funnel every loop's
lifecycle posts into whichever single channel ralphd's own environment names.
Give each `[[loop]]` a `webhook` pointing at its channel; a loop with none runs
with the variable *cleared* rather than inheriting (a lone loop still inherits
the ambient one, so single-loop deployments are unchanged).

Commands — each acts on the loop that owns the channel you type it in:

| Command | Shells out to |
|---|---|
| `/start [model]` | `ralph <profile args> [--model …]` |
| `/stop [now]` | `ralph stop` / `ralph stop --now` |
| `/model <tier>` | `ralph model <tier>` |
| `/status`, `/next` | `ralph status --json` |
| `/add <title> [verify] [id] [under]` | `ralph add [--under P] [id] <title> [--verify …]` |
| `/drop <id> [recursive]` | `ralph drop <id> [--recursive]` |
| `/uncheck <id>`, `/done <id>` | `ralph uncheck <id>`, `ralph done <id>` |
| `/backlog-edit <id> <title> <verify>` | `ralph backlog edit …` |
| `/msg <message> [new]` | `ralph msg [--new] <text>` |

`/start` takes an optional model to override the launch default for one run.
`/msg` steers the loop through its *persistent* claude session (`.ralph/`-backed,
so you can start it from your phone and continue from a terminal on the same
context) and streams its progress back. Invite the bot with the `bot` +
`applications.commands` scopes (pinning the status card also needs the *Manage
Messages* permission; without it the card degrades to an ordinary message).

Beyond commands, ralphd maintains channel state on its own:

- **One pinned live status card per channel**, edited every 30s while that loop
  runs: loop name, run-state, iteration, pending count, current + upcoming
  leaves, the live in-iteration line from `.ralph/live`, spend from
  `.ralph/ledger.jsonl`, and a relative "updated" stamp. When the loop ends the
  card gets a final past-tense edit and stays as the run's record; the next run
  deletes it and pins a fresh one — exactly one card, never a pile of status
  posts.
- **A budget warning at 80%.** Spend is the sum of *every* ledger line over the
  repo's `budget_window` (a LIMIT retry appends a second line for the same
  iteration, and that money was really spent), compared against the same
  `budget_usd` from that repo's `ralph.toml` that `ralph` enforces — ralphd only
  surfaces it. No ledger file, or no configured budget, and the line is simply
  absent.
- **Actionable failure posts**: when a ralphd-spawned loop exits abnormally,
  ralphd posts the abort reason (pulled from `run.log`) with **Start again** /
  **Start on opus** buttons — the "come look" signal carries its remedies, so
  you can unblock from your phone. Buttons pass the same auth gate as commands.
- **`/msg` message hygiene**: output is never truncated — it's split into up to
  4 messages at line boundaries, code fences are closed and reopened across the
  split (never torn), and all mentions are suppressed so a session can't ping
  `@everyone`. The first status edit lands within ~20s and progress updates
  every minute; past 14 minutes the live message migrates off the interaction
  token (which Discord expires at 15) into a plain channel message.

`ralph` owns `loop.pid`: ralphd reads it to answer "is this channel's loop
running" and to refuse a duplicate `/start`, but never writes it.

## Completion
The loop ends when the model's **final text** (from the result envelope's
`.result`, which excludes thinking) contains the marker token on its own line,
default `RALPH_COMPLETE`. Your `PROMPT.md` must instruct the model to emit it
only when the whole goal is genuinely done and verified.

### Completion closes the arc
On completion, the runner moves the backlog file into
`.ralph/archive/BACKLOG-<timestamp>.md` — `git mv` + a commit when the backlog
is tracked, a plain filesystem rename otherwise — and then closes out the arc:

- the carry-forward is archived to `.ralph/archive/PROGRESS-<timestamp>.md` and
  `PROGRESS.md` is cleared, so the next arc's first iteration never reads the
  previous arc's notes;
- the iteration counter resets to 0, so a fresh `--max-iterations` budget means
  what it says (the counter's persistence is for resuming *within* an arc;
  post-completion there is nothing to resume);
- `.ralph/learnings/` is deliberately untouched — that's the memory that should
  survive arcs.

All best-effort: a finished run is never turned into a failure by archive
hiccups.

### Starting the next arc
`ralph add` bootstraps a fresh, schema-valid `BACKLOG.md` when the file
is absent, so the whole cycle works without touching a terminal: complete →
`/add …` (repeat as needed) → `/start`. Edit `PROMPT.md` between arcs
when the goal or verification contract changes; config, learnings, and the
webhook carry over as-is.

## Per-iteration hand-offs (the agent writes these)
Each iteration ends by writing **one** consolidated report, `.ralph/HANDOFF.json`:

```json
{"status": "code", "model": null, "blocked": null}
```

- `status` — this iteration's type: `code` (a normal committing iteration), or
  `review`/`plan`/`blocked` for an intentional non-code pass. Absent is treated
  as `code`.
- `model` — `haiku` / `sonnet` / `opus`, a **one-shot override** sizing the NEXT
  iteration; cleared once read. Normally null: a task's own `(tier/…)`
  decoration is the baseline (see below).
- `blocked` — with `status: blocked`, one line naming exactly what a human must
  clear; it is logged and posted to the webhook so the "come look" signal
  carries its reason.

One file instead of the previous `STATUS`/`MODEL` pair, so the agent can't
half-comply; the legacy files are still honored when no handoff is present.
Malformed JSON or invalid field values are warned about and ignored (never
abort). See the PROMPT template for the exact instructions given to the model.

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

## Mechanical contract audit (prompt-as-request, runner-as-contract)
After every successful iteration the runner audits the commit/safety contract
the PROMPT only *requests*, using git itself:

- **Branch switched** → the loop's core invariant is gone: logged, posted to the
  webhook, and the loop **aborts** immediately.
- **History rewritten** (HEAD moved without fast-forward: amend/reset/rebase) →
  logged + posted, and the iteration **counts as no-progress**.
- **Runtime files committed** (any `.ralph/` path in the new commits) → same.

Lenient outside a git repo. This closes the gap where a prompt rule is only as
strong as the model's compliance.

## Adversarial check-off judge (opt-in)
For expensive tiers it's worth a second opinion before a check-off stands. With
`judge_tiers = ["opus"]` in `ralph.toml` (default: off), every committed `code`
iteration that RAN on a listed tier gets a one-shot judge pass on
`judge_model` (default `sonnet`): the judge reads the leaf's own text (with its
`Verify:` contract), the agent's end-of-turn summary (labeled *claims,
unverified*), and the iteration's commits + diff, and is told to **refute if
uncertain**. On refute, the runner mechanically un-checks the leaf (and any
ancestor the check-off closed) with the usual lint-or-reject safety, posts the
reason, and counts the iteration as no-progress — so routing re-selects the
same leaf, and a repeat refutation escalates the tier like any other stall.
The harness itself fails **open**: a missing/hung/garbled judge call passes the
iteration rather than stalling the loop (skepticism belongs in the judgment,
availability in the harness).

## `ralph learn` — durable lessons as files
Mines `run.log` (plus the current carry-forward) with a one-shot `synth_model`
call for durable, non-obvious lessons: recurring failures, environment gotchas,
verification traps, tier lessons. Propose-then-approve, never auto-written:

```bash
ralph learn              # mine → print numbered proposals (saved, not applied)
ralph learn --apply      # write all proposals to .ralph/learnings/<slug>.md
ralph learn --apply 1,3  # write a subset
ralph learn --discard    # drop the saved proposals
```

Discipline: existing learnings are shown to the miner so they aren't
re-proposed, and "nothing non-obvious happened" yields an empty proposal list —
that's the correct outcome for a clean run, not a failure. One learning per
file so cleanup is `rm .ralph/learnings/<file>`.

Learnings are injected into every iteration under a `## Learnings` heading,
appended to the stable base prompt (cache-friendly: they change only when you
apply or prune). Budgets: 1 KB per file, 4 KB total; files over budget are
named in the prompt rather than silently dropped.

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
| Persisted spend | `budget_usd` + `budget_window` (toml only) | 0 (off) |

`--max-cost` counts one process's spend, so a restart hands the loop a fresh
allowance. `budget_usd` instead sums `.ralph/ledger.jsonl` — one
`{"ts","iter","model","cost_usd"}` line appended per iteration — over the
trailing `budget_window` (`"24h"`, or bare seconds; unset = all time), so it
survives restarts. A non-finite envelope cost is recorded as `0.0`; an
unparseable line contributes nothing rather than fabricating a halt.

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
| `judge_tiers` | `RALPH_JUDGE_TIERS` (comma-sep) | — | `[]` (off) |
| `judge_model` | `RALPH_JUDGE_MODEL` | — | `sonnet` |
| `budget_usd` | — | — | `0` (off) |
| `budget_window` | — | — | `0` (all time) |
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
`state` (`.ralph/`, HANDOFF) · `git` (baseline, contract audit) · `judge`
(adversarial check-off gate) · `learn` (`ralph learn`) · `init` (`ralph init`
scaffolding). See
`docs/superpowers/specs/2026-07-17-ralph-rust-design.md` for the original design.
