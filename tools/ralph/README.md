# ralph — external autonomous loop

Runs `claude -p` or OpenAI’s `codex exec` in a loop, **fresh context each iteration**, feeding a stable
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
toolchain; the selected `claude` or `codex` CLI must be authenticated on PATH at runtime):

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
2. Flesh out `.ralph/BACKLOG.md` using the v2 schema, optionally add a VISION.
   PROGRESS is runner-owned — no need to seed it; the orchestrator writes a
   carry-forward note there after each iteration.
3. `ralph init` already wrote the `.gitignore` block for you (see below) — no
   manual step needed.
4. Check routing, then run it on a dedicated branch:

   ```bash
   ralph schema
   ralph lint
   ralph brief --full
   ralph doctor
   ralph --max-iterations 30      # from the repo root
   ```

   Test a single pass first with `ralph --once`.

Run **one `ralph` per worktree** — each loop drives the repo it is launched in.

### Choose a backend and model

```bash
ralph --model opus --once                 # Claude Opus
ralph -m gpt-5.4 --once                    # OpenAI via Codex (auto-detected)
ralph --backend codex --model gpt-5.4      # explicit backend
ralph --backend codex                     # use Codex’s configured default model
ralph model gpt-5.4                       # one-shot override for the next iteration
ralph msg --backend codex --model gpt-5.4 "review the plan"
```

`--model` (also `-m`) accepts a tier alias or a concrete model identifier.
It sets the run default; task decorations and escalation still take precedence,
as before. `ralph model <name>` is the one-shot override and can select `opus`
even when it is absent from the escalation ladder. Model availability is checked
by the backend CLI, so new model names do not require a Ralph release.

`backend` defaults to `auto`: `gpt-*`, `chatgpt-*`, `codex-*`, and `o1`/`o3`/`o4`
model names (including suffixed variants) select Codex; other names select Claude.
The short aliases `astra`, `sol`, `terra`, and `luna` also select Codex.
`fable` selects Claude Fable 5.1 (`claude-fable-5-1`); `astra` selects
GPT-6 Astra (`gpt-6-astra`). Explicit versioned IDs are passed through unchanged.
Use `--backend codex` for custom OpenAI model names. `openai` is an alias for
`codex`, and `anthropic` for `claude`. An explicit backend governs the run’s
worker, synthesizer, judge, and learning calls. Authentication comes from the
selected CLI’s existing login.

The **backlog schema stays v2**. Its `@haiku`, `@sonnet`, and `@opus` decorations
remain the low, medium, and high tiers. On Codex, an unmapped tier uses the run’s
concrete model (or Codex’s configured default) with that tier’s reasoning effort.
You can optionally assign concrete models to tiers in `.ralph/ralph.toml`:

```toml
backend = "codex"
model = "gpt-5.4"
effort = "auto"

[tier_models]
haiku = "gpt-5.4"
sonnet = "gpt-5.4"
opus = "gpt-5.4"
```

Replace those model IDs with models available to your account when you want
escalation to change the model as well as effort. With no mapping, Claude’s
existing tier behavior is unchanged. `synth_model` and `judge_model` accept the
same tiers or model IDs and also have `--synth-model` / `--judge-model` flags on
loop launches. `effort = "inherit"` uses the CLI’s settings; Codex translates
Ralph’s `max` effort to `xhigh`.

Codex iterations use `exec --json --ephemeral`; steering sessions use persistent
threads and `exec resume`. Native Codex events are kept in iteration logs and
converted to Ralph’s existing `last-result.json` envelope. Completion uses only
the last completed assistant message, excluding reasoning and tool output.
See the official [Codex noninteractive interface](https://developers.openai.com/codex/noninteractive/).

Codex’s documented event stream reports tokens but **does not report USD cost**.
Its ledger entries retain the existing schema with `cost_usd = 0` meaning
unreported, not free. Ralph rejects `max_cost_usd` / `budget_usd` on Codex turns
rather than silently ignoring a budget. Use iteration and wall-clock limits.
`fallback_model` is Claude-only because Codex has no equivalent flag.
`extra_args` are passed to the selected worker CLI unchanged; use arguments that
CLI supports. `--no-yolo` runs Codex in `workspace-write` with approvals disabled
for unattended operation; otherwise Ralph uses Codex’s bypass flag. Codex helper
calls use a read-only sandbox.

### Exclusive model selection

Prefix a model with `!` to require that model for the call. For example:

```bash
ralph --model '!astra' --once
ralph model '!fable'
ralph msg --model '!astra' "review the plan"
```

The same syntax works in model configuration values and in the task's header:

```markdown
- [ ] **12 — Review the architecture.** !fable — use Fable exclusively.
  Verify: cargo test
```

`@!fable` is also accepted. Exclusive selections choose the model's provider,
overriding a conflicting `backend`, and disable automatic provider failover and
Claude's configured overload fallback. Usage limits retain the existing wait/retry
behavior on that provider. An exclusive task annotation outranks escalation and
one-shot overrides; no-progress limits still apply. Helper models are configured
separately and can also use `!`. Plain `@astra` and `@fable` permit failover.

### Automatic provider failover

Failover is **on by default**. When a worker reports depleted usage, quota, or
credits, Ralph immediately retries the task on the other provider using its
existing CLI login. The default pairings work in both directions:

| Anthropic | OpenAI |
|---|---|
| Fable (`claude-fable-5-1`) | Astra (`gpt-6-astra`) |
| Opus | Sol (`gpt-5.6-sol`) |
| Sonnet | Terra (`gpt-5.6-terra`) |
| Haiku | Luna (`gpt-5.6-luna`) |

Versioned family names use the same pairing. Unknown models use Sonnet/Terra;
OpenAI mini models use Haiku. These are routing defaults and can be overridden.
Ralph reads reset hints from worker errors, including `try again in 2h 15m`,
`Retry-After: 60`, ISO timestamps, and `resets 5pm (America/Chicago)`.
Dated reset times and named timezones are honored; clock times without a zone
use the machine's local timezone. Unrecognized, invalid, or stale hints fall
back to configured waits.

Anthropic and OpenAI have independent deadlines and backoff counters, saved in
`.ralph/provider-limits.json` as Unix UTC seconds (`retry_at`). Restarting Ralph
preserves these timers. Workers use the available provider and return to the
preferred provider when its deadline expires. If both providers are blocked,
Ralph waits until the earliest usable provider resets, without probing either
early. Logs and `.ralph/live` show when the next retry is scheduled. Long limit
waits honor STOP and the wall-clock budget. One-shot model choices survive
quota retries within the loop process, and logs and spend records identify the
model that actually ran.

Without a reset hint, depletion uses `failover_cooldown` (30 minutes by default)
when an alternate provider is usable. Otherwise, limits use capped exponential
backoff (`limit_wait` → `limit_wait_max`), tracked independently per provider.
Explicit reset hints take precedence over these fallback waits, including their
caps. Plain rate limits (including a bare HTTP 429) wait on the same provider;
network and authentication errors keep their existing handling. A missing
alternate CLI, disabled failover, or an active USD budget that cannot be enforced
on Codex excludes that alternate from scheduling.
CLI-specific `extra_args` are omitted when switching providers; Ralph preserves
the permission mode and translates reasoning effort.

Helper calls and `ralph msg` also try the other provider once on depletion.
A message switch starts a conversation using project files; provider-specific
conversation history is not transferred. The successful provider/model is saved
for subsequent messages; failure preserves the old session. Each helper attempt
has its own timeout.

No configuration is required. Optional settings in `.ralph/ralph.toml`:

```toml
# provider_failover = false  # opt out (also --provider-failover false)
# failover_cooldown = "30m"  # fallback when depletion output has no reset time

# Exact source → destination overrides; add both directions if desired.
# [failover_models]
# opus = "gpt-5.6-sol"
# "gpt-5.6-sol" = "opus"
```

An explicit `--backend` chooses the preferred provider; automatic failover still
applies unless disabled. `fallback_model` remains Claude's separate overload
fallback setting.

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
context the model will receive. That reference is
[BACKLOG.schema.md](BACKLOG.schema.md), compiled into the binary — editing it
takes effect only after a rebuild.

The runner curates as it goes: after each successful iteration the maximal
leading run of fully-completed top-level sections is lifted out of the live
backlog and appended to `.ralph/archive/BACKLOG-completed.md`, keeping the file
scoped to pending work. Selection is pure document order, so a prefix lift
changes no routing. Best-effort and conservative — it never touches an invalid
backlog, and never leaves one behind.

## Watching / controlling a running loop
- **Live status of the active iteration** (tool, elapsed, output tokens, last
  activity): `cat .ralph/live`
- **Raw stream of the active iteration** (includes thinking): `tail -f .ralph/current.log`
- **High-level progress:** `tail -f .ralph/run.log`
- **Stop gracefully and wait:** `ralph stop` writes STOP and waits until the
  recorded loop exits after its current iteration (including acceptance and handoff).
- **Request a stop without waiting:** `ralph stop --async` returns immediately.
  With no live loop, either form returns immediately and leaves STOP for the next launch.
- **Halt immediately and wait for teardown:** `ralph stop --force` also sends
  SIGTERM; the runner kills the active worker, helper, or verification process group.
  `--now` remains an alias. Add `--async` to return after sending the signal.
  Both graceful and forced stops suppress `--restart`. Force may leave partial work.
- **Start via ralphd** without Discord: `ralph start` writes `.ralph/START`, the
  symmetric counterpart to STOP. A running ralphd consumes the marker and
  launches the loop; with no ralphd watching, nothing happens.
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
- `ralph model <name>` — write the one-shot `.ralph/MODEL` override consumed by
  the next iteration. Accepts tiers and concrete model IDs; tier aliases are
  trimmed and matched case-insensitively. Supports `--dir` and `--config`.
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
`ralph msg "<text>"` talks to an agent session attached to this repo's loop,
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
  full name like `claude-fable-5-1`) and is *not* checked against the escalation
  ladder, which governs the loop rather than this session.
- Only one `msg` runs at a time; a second is refused, not queued (`.ralph/msg.pid`).

The session state lives in `.ralph/` and retires together:

| File | Holds |
|---|---|
| `.ralph/msg-session` | the Claude session or Codex thread id (a UUID) |
| `.ralph/msg-model` | the sticky `--model` pin, absent when unset |
| `.ralph/msg-backend` | the session’s backend; absent on legacy Claude sessions |

Session state is written only after the backend returns a successful result, so a failed first call cannot
leave behind an id that every later resume fails against, nor a rejected model
name that poisons every later message. `--new` archives the id into
`.ralph/archive/` and clears the pin; completing an arc does the same. If a pin
seems stuck, `cat .ralph/msg-model` is the whole story.

The session’s backend stays with the conversation, including when repinning a
custom model name. An explicit `msg --backend` or a recognized concrete model
from another family selects a different backend; loop config supplies the
backend for new conversations. Changing a steering session’s backend starts a
fresh conversation and archives the previous session only after the new call
succeeds. `--stream-json` keeps
the existing assistant/result event format for consumers such as ralphd.

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
| `/stop [now]` | `ralph stop --async` / `ralph stop --force --async` |
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
default `RALPH_COMPLETE`, **and** the backlog agrees. Your `PROMPT.md` must
instruct the model to emit it only when the whole goal is genuinely done and
verified.

Completion follows the Git contract audit, any configured acceptance policy, and
reconciliation of queued mutations. The runner re-resolves the backlog, and a marker
that arrives while a pending task or a schema error remains is discarded with
`⚠ completion marker ignored: <reason>` and the loop simply continues.

### Completion closes the arc
On completion, the runner moves whatever backlog remains into
`.ralph/archive/BACKLOG-<timestamp>.md` — a plain filesystem rename that never
touches git — and then closes out the arc:

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
- `model` — a tier (`haiku` / `sonnet` / `opus`) or concrete model ID, a **one-shot override** sizing the NEXT
  iteration; cleared once read. Normally null: a task's own `@tier`
  decoration is the baseline (see below).
- `blocked` — with `status: blocked`, one line naming exactly what a human must
  clear; it is logged and posted to the webhook so the "come look" signal
  carries its reason.

One file instead of the previous `STATUS`/`MODEL` pair, so the agent can't
half-comply; the legacy files are still honored when no handoff is present.
Malformed JSON or invalid field values are warned about and ignored (never
abort). See the PROMPT template for the exact instructions given to the model.

**Model precedence** (highest first): escalation override → one-shot `.ralph/MODEL`
→ the resolved leaf's own `@tier` decoration → the run default. So model tier
lives with the task in the backlog; the agent need not restate it. An active
escalation short-circuits the rest, so a pending `.ralph/MODEL` is *not* consumed
while one holds — it survives to the next non-escalated iteration.

The decoration sits in a fixed slot on the header line — immediately after the
label's closing `**`, closed by ` — ` before the prose:

```markdown
- [ ] **12 — Rework the shared base.** @opus — big, cross-cutting change.
```

Tier decorations (`@haiku`, `@sonnet`, `@opus`), known model families and aliases
(such as `@astra` or `@fable`), and exclusive selections (`!astra`, `!fable`)
are accepted, at most one per task. A task
with no decoration starts its prose right after the ` — `. Because the slot is
positional, an `@opus` anywhere else in the body is inert prose, and the
decoration cannot wrap onto a second line. Anything malformed in the slot — an
unknown token, a missing ` — `, a second tier — is a hard `ralph lint` error
that refuses the iteration, never a silent fall back to the default model.

Before every process launch the runner parses the complete backlog, selects the
next leaf by document order, and appends a bounded brief containing that
leaf plus PROGRESS's carry-forward note injected verbatim — no `Next:`
parsing, no id matching. The base prompt remains first and stable for caching.
For Claude, Ralph also passes `--no-session-persistence` (iterations are deliberately
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
ancestor the check-off closed) with the usual lint-or-reject safety, **discards
the agent's own queued `ralph done` for that leaf**, posts the reason, and counts
the iteration as no-progress — so routing re-selects the same leaf, and a repeat
refutation escalates the tier like any other stall. That discard is what makes
the reopen stick: the agent closes its leaf through the same queue as every other
CLI mutation, so at judge time the check-off is still pending rather than applied,
and draining it at the next iteration boundary would silently re-close the leaf
the judge just reopened.
Legacy tier judging still fails **open** on availability, but records a missing,
hung, or garbled judge result as **unavailable**, never as a pass. Task-local
`review = "required"` makes availability mandatory; `review = "advisory"` records
critiques without blocking (unless legacy `judge_tiers` also requests judgment).
The judge uses the frozen task contract and commits since that task began.
Refutation reasons are injected directly into the next attempt, independently of
the worker's summary.

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
was a transient/timeout retry. A first declared non-`code` pass (`review`/`plan`) is excluded. Repeated
non-code passes that change neither the backlog nor the committed product tree
count as no-progress starting with the second unchanged pass. A commit changing
neither the product tree nor the backlog also counts as no-progress. On the streak reaching:

- `--escalate-after` (default 2): the model escalates one tier up the ladder
  `haiku → sonnet → opus` for the next attempt;
- `--abort-after` (default 4): the loop aborts with a clear reason.

A productive `code` iteration resets the streak. Streaks, escalation, task
attempt counts, and the initial task revision persist in `.ralph/thrash.json`.
Task or contract changes reset them. Four attempts on the same task produce a
split/reframe diagnostic even if commits continue. `task_attempt_limit = N`
optionally caps attempts against one unchanged task contract; the default `0`
only diagnoses, allowing fuzzy or incremental work to continue. Quota retries
are excluded. To deliberately clear the detector after an external intervention,
stop the loop and remove `.ralph/thrash.json` before restarting.

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
needed) and the loop posts lifecycle events to it: start, each iteration's
launch and its one-line result (cost, tokens, turns, timing, summary), model
escalation, contract breaches, abort (no-progress **or** a hard `blocked` gate —
the "come look" signal), completion, and any budget/STOP halt. Unset = disabled.
Posts go out via `curl` with a 10s timeout and all errors swallowed, so a slow or
down webhook never stalls or fails the loop. For the raw stream of a turn in
flight, watch `.ralph/current.log`.

`--heartbeat <dur>` (`heartbeat_interval`, off by default) adds *in-turn*
progress posts every interval while an iteration runs — elapsed, output tokens,
event count, current tool — so a long turn is visibly alive. It posts from its
own thread, so a slow webhook never stalls stream consumption, and it is inert
without a webhook.

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
| **LIMIT** | 429, or text matching `usage limit` / `credit balance` / `quota` / `will reset` / `rate limit` | Honor the provider reset time, or use configured cooldown/backoff when unknown. Independent persisted provider timers; retry the earliest usable provider. Unlimited retries; never counts as no-progress. |
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
| `backend` | `RALPH_BACKEND` | `--backend` | `auto` |
| `tier_models` | — | — | `{}` (optional tier → model mapping) |
| `model` | `RALPH_MODEL` | `--model` / `-m` | `sonnet` |
| `fallback_model` | `RALPH_FALLBACK_MODEL` | `--fallback-model` | `sonnet` |
| `provider_failover` | `RALPH_PROVIDER_FAILOVER` | `--provider-failover` | `true` |
| `failover_cooldown` | `RALPH_FAILOVER_COOLDOWN` | `--failover-cooldown` | `1800` (30m) |
| `failover_models` | — | — | `{}` (overrides default provider pairings) |
| `synth_model` | — | `--synth-model` | `sonnet` |
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
| `restart` | `RALPH_RESTART` | `--restart` | `false` |
| `heartbeat_interval` | `RALPH_HEARTBEAT` | `--heartbeat` | `0` (off) |
| `limit_wait` / `_max` | `RALPH_LIMIT_WAIT[_MAX]` | — | 300 / 3600 |
| `transient_wait` / `_max` | `RALPH_TRANSIENT_WAIT[_MAX]` | — | 10 / 300 |
| `extra_args` | `RALPH_EXTRA_ARGS` | — | — |
| `escalation_ladder` | — | — | `["haiku","sonnet","opus"]` |
| `judge_tiers` | `RALPH_JUDGE_TIERS` (comma-sep) | — | `[]` (off) |
| `judge_model` | `RALPH_JUDGE_MODEL` | `--judge-model` | `sonnet` |
| `budget_usd` | — | — | `0` (off) |
| `budget_window` | — | — | `0` (all time) |
| — | `RALPH_CONFIG` | `--config` | `.ralph/ralph.toml` |
| — | — | `--once` | run one iteration then exit |

`escalation_ladder` and `judge_tiers` may name only `haiku`, `sonnet`, or
`opus` — the tiers a backlog `@tier` slot can spell. Anything else is a startup
error, not a silently inert entry. `tier_models` uses the same three keys but
accepts concrete model IDs as values; it does not change the backlog grammar.

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
- The selected `claude` or `codex` CLI on PATH (authenticated).
- The Rust toolchain to build (via the personalize script).
- Python 3 and git to run the offline subprocess integration tests.

## Development
```bash
cargo test          # backlog/context/config/stream/state/git/thrash
cargo build --release
```
Modules: `backend` (CLI/model routing) · `backlog` (schema/lint) · `context` (bounded brief) · `config` ·
`stream` (NDJSON) · `classify` · `control` (loop, thrash, budgets, timeout) ·
`state` (`.ralph/`, HANDOFF) · `curate` (completed-section sweep) · `git`
(baseline, contract audit) · `judge` (adversarial check-off gate) · `learn`
(`ralph learn`) · `init` (`ralph init` scaffolding). See
`docs/superpowers/specs/2026-07-17-ralph-rust-design.md` for the original design.


## Proportional acceptance (opt-in)

`Verify:` remains prose: a qualitative success criterion, an observable outcome,
or instructions for a targeted check are all valid. Ralph never executes that
text as shell code. Ordinary tasks make no extra verification or review call.
Cheap Git invariants apply to every successful turn, including the terminal turn.

For tasks that benefit from a runner-executed gate, configure their IDs in the
local TOML. A parent ID applies when the parent becomes the integration task;
it is not automatically run for each child.

```toml
# Optional, root-level setting; 0 keeps task-attempt limits advisory.
task_attempt_limit = 0

[acceptance."3.2"]
command = ["./tools/verify-parser", "--focused"]
timeout_secs = 120

[acceptance."4"]
review = "advisory"  # critique recorded; does not block acceptance

[acceptance."5"]
review = "required"  # refuted or unavailable review prevents acceptance

[acceptance."@complete"]
command = ["./tools/integration-smoke"]
timeout_secs = 300
```

Commands are argv arrays, run from the loop's working directory with
`RALPH_TASK_ID` set, no interactive stdin, and separate captured stdout/stderr.
Use an explicit shell invocation if shell syntax is needed. The timeout kills
the command's process group. Policies are loaded with the launch configuration;
the selected task's full contract, including ancestor constraints, is frozen
before dispatch. A gate runs only when that task requests closure. Existing
`judge_tiers` keeps its earlier per-committed-turn behavior.

A failed command or required review refutation reopens the selected task and
cancels its queued check-off (and ancestor check-offs). It feeds no-progress
tracking. Unavailable required review halts with a clear reason instead of
paying for repeated worker attempts. Advisory review never blocks by itself.
A configured `@complete` policy runs in the final audit turn after the last task
closes, rather than on each iteration.

These policies govern worker acceptance within a run. A manual `ralph done`
while the loop is stopped remains an operator action, without launching checks.

## Failure feedback and run records

`.ralph/previous-attempt.json` stores runner-observed rejection or execution
failure evidence: task ID, contract fingerprint, reason, revision, and artifact
path. Matching feedback is injected verbatim (up to 4 KB), independently of
synthesized notes; it cannot reroute the task. Changed contracts omit stale
feedback. Successful acceptance clears it. Quota retries preserve the prompt.

Carry-forward notes are limited to 1,200 bytes on synthesis success **and**
fallback. The context reader also bounds manually enlarged progress files and
indicates truncation. Complete worker summaries remain in the run artifacts.

Each launch has a unique `.ralph/runs/<run-id>/` directory. It contains `run.json`
and an `attempt-NNNN/` directory per worker invocation, including retries:

- `prompt.md`, `config.json`, `contract.md`, and driving-file snapshots;
- `git.json` with the starting branch/revision;
- `worker-summary.md`, `worker-result.json`, and a pointer to the raw worker log;
- verification command, revision, exit status, timeout, and output when configured;
- review prompt, raw response, and pass/refuted/unavailable verdict when configured;
- `outcome.json`, `accepted.json` for accepted closures, and post-turn backlog.

The configured webhook is omitted from configuration snapshots. These are local,
gitignored diagnostic artifacts. They are retained until you prune them.

`.ralph/run.json` is the latest run record, atomically replaced. `ralph status
--json` adds `running`, `run`, and `diagnostics` while preserving the existing
backlog fields. It reports worker/provider/model, phase, task attempts, artifact
path, last accepted revision, worker cost (with an unreported-cost flag), and
terminal reason. Phase distinguishes work, verification, review, handoff, quota
wait, and transient retry. Status works after backlog archival and exposes
invalid-backlog diagnostics. ralphd displays the same phase and outcome.

A dead process without a terminal record is shown as interrupted. On restart,
an interrupted attempt without an acceptance receipt has its check-off revoked
and gets explicit recovery feedback. Product changes are preserved for inspection;
Ralph does not automatically reset or restore the worktree. Existing spend-budget
semantics are unchanged: helper costs are still outside the worker ledger.

## Preflight and exact prompt preview

`ralph doctor [--json]` checks the current worktree/branch, runtime path, backlog,
prompt placeholders, selected model, executable availability, permission mode,
budget compatibility, and configured verification commands. It does not call a
model, probe authentication, or run verification. Unknown task IDs are warnings
(they may refer to an archived task or a later arc). Errors exit 1; warnings alone
exit 0. It accepts the normal configuration/path flags.

`ralph brief --full` writes the composed worker prompt to stdout: base prompt,
learnings, authoritative task context, bounded carry-forward, matching failure
feedback, and explicit acceptance policy. Size/truncation information goes to
stderr. It uses the same composition function as launch and does not consume
model overrides, drain queued mutations, or write runtime state. The preview
reflects the current files; queued edits appear after the next boundary.
