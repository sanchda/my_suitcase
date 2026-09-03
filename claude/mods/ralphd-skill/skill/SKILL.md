---
name: ralphd
description: Use when setting up, configuring, or debugging ralphd — the Discord control bridge for ralph loops — including writing or fixing ralphd.toml, inviting/scoping the bot, slash commands that never register or silently vanish, driving a ralph loop from Discord (/start /stop /status /msg), the pinned status card not appearing or not updating, budget warnings on the card, or abnormal-exit restart buttons.
---

# ralphd — the Discord bridge for ralph loops

One always-on foreground process watching **one guild**. Every command shells out to the
`ralph` binary (bare name, from PATH) and ralphd only *reads* `.ralph/`, so it is **never
load-bearing**: killing it loses nothing, and anything doable from Discord is doable from a
terminal. For the loop itself — backlog schema, budgets, escalation — use the **ralph**
skill; this one is only the bridge.

**The channel selects the loop.** No `--repo` argument: `/status` in `#number-grove` means
that repo because a `[[loop]]` claims that channel id. One channel drives one loop; two
loops on one channel is a parse error.

## Auth model (`src/auth.rs`, in full)

```rust
user_id == cfg.user_id && cfg.loops.contains_key(&channel_id)
```

One guild, one user, N channels. No roles, no per-command permissions. Anything else gets an
ephemeral `not authorized in this channel` and nothing runs. Button clicks pass the
identical gate. Successful replies are ordinary channel messages, **non-ephemeral on
purpose** — the channel is the audit trail of what was run. ralphd runs shell commands as
you in your repos, so the bot token and the user id are the only fence there is.

## Install and run

```bash
<suitcase>/personalize/scripts/setup_ralph.sh   # builds ralph + ralphd → ~/.local/bin
DISCORD_BOT_TOKEN=… ralphd --config ~/.config/ralphd.toml
```

Both binaries come from the suitcase repo (`tools/ralph`, `tools/ralphd`); re-run that
script after pulling. `ralph` must be on PATH — ralphd invokes it by bare name.

Runs in the foreground; exits 2 on a config error with the reason plus usage.
Run `ralphd --help` for the exhaustive flag/env reference — read it, don't guess.

## Config file

Copy the suitcase's `tools/ralphd/ralphd.toml.example`; every accepted key is in it.

```toml
guild = 123456789012345678       # required (or RALPHD_GUILD_ID; the file wins)
user  = 234567890123456789       # required (or RALPHD_USER_ID; the file wins)

[[loop]]
name      = "number-grove"                          # required — card label + log tag
channel   = 345678901234567890                      # required — unique across loops
dir       = "/home/me/dev/number_grove"             # required — ralph's cwd
args      = ["--model", "sonnet"]                   # optional — launch profile
webhook   = "https://discord.com/api/webhooks/…"    # optional — per loop, see TRAPS
autostart = false                                   # optional, default false
```

Hard errors, all exit 2, none silent: any unknown key (`deny_unknown_fields` on both
tables), no `[[loop]]`, two loops on one channel, `channel = 0`, an unreadable `--config`
path, a missing/whitespace `DISCORD_BOT_TOKEN`, `guild` or `user` absent from both file and
env. `autostart` launches on gateway connect, skipping any loop whose `loop.pid` is already
live, and is latched so a reconnect never starts a second loop.

`args` is forwarded verbatim to `ralph` on `/start`, and a `--dir` / `--config` in it also
tells ralphd where to *read* that loop's state and `ralph.toml` (relative paths resolve
against `dir`; defaults `<dir>/.ralph` and `<dir>/.ralph/ralph.toml`). The short shell-outs
(`ralph status --json`, `add`, …) run with cwd `dir` and carry that relocation as
`RALPH_DIR` / `RALPH_CONFIG` on the child, so they act on the loop's own state dir.
Re-passing `--dir` would not do it: `ralph` takes that flag from argv only for the loop,
`stop` and `msg` — `status`, `add`, `model` and `backlog edit` resolve their paths from the
config file and the environment. A `--backlog` travels the same way as `RALPH_BACKLOG`;
`ralph` resolves `backlog` independently of `dir`, so moving one never moves the other.
`args` that relocate nothing set neither var, so
`dir = "…"` in the repo's `ralph.toml` still decides — but the card never reads that key and
watches `<dir>/.ralph` regardless, so relocate with `--dir` in `args`.

**Precedence, first match wins:** (1) `--config` / `RALPHD_CONFIG`, (2) the flag form,
whenever `--channel` / `RALPHD_CHANNEL_ID` is set, (3) `~/.config/ralphd.toml` if it exists.
The flag form beating the default path is deliberate — dropping a config file on the box
must not hijack a launch that already works.

## Single-loop flag form

```bash
DISCORD_BOT_TOKEN=… ralphd --guild <id> --channel <id> --user <id> \
  [--working-dir <repo>] [--autostart] -- <ralph args forwarded to /start>
```

Every flag has an env twin and the **flag wins**: `RALPHD_CONFIG`, `RALPHD_GUILD_ID`,
`RALPHD_CHANNEL_ID`, `RALPHD_USER_ID`, `RALPHD_WORKING_DIR`, `RALPHD_AUTOSTART` (truthy =
anything but empty, `0`, `false`). `--working-dir` defaults to `.` and names the loop after
its basename. **`DISCORD_BOT_TOKEN` is env-only, never a flag.**

## Commands → `ralph` invocations

| Discord | Shells out to |
|---|---|
| `/start [model]` | `ralph <loop args> [--model …]` — appended, so it wins for one run |
| `/stop [now]` | `ralph stop [--now]` |
| `/model <tier>` | `ralph model <tier>` |
| `/status`, `/next` | `ralph status --json` (identical code path) |
| `/add <title> [verify] [id] [under]` | `ralph add [--under P] [id] <title> [--verify …]` |
| `/drop <id> [recursive]` | `ralph drop <id> [--recursive]` |
| `/done <id>`, `/uncheck <id>` | `ralph done` / `ralph uncheck <id>` |
| `/backlog-edit <id> <title> <verify>` | `ralph backlog edit --id … --title … --verify …` |
| `/msg <message> [model] [new]` | `ralph msg [--new] [--model …] --stream-json <text>` |

**All validation lives in `ralph`** — tiers, ids, backlog schema. ralphd echoes stdout on
success or `rejected: <stderr>` on failure, so debug refusals against `ralph`, not ralphd.
Blank options are dropped, never passed as empty strings. `/start` refuses while `loop.pid`
names a live pid.

## The pinned status card

One pinned message per channel, edited in place every **30s** (so it appears up to 30s after
the loop starts): name + pid, run state, iteration, pending count, current and upcoming
leaves, the first line of `.ralph/live`, spend from `.ralph/ledger.jsonl`, and a relative
`updated` stamp. On exit it takes one final past-tense `— ended` edit, drops the stale live
line, and stays as that run's record; the next run **deletes** it (which unpins) and pins a
fresh one — one card per channel, never a pile. Pinning is best-effort: without Manage
Messages the card degrades to an unpinned message and keeps updating.

**Budget warning at 80%.** `budget_usd` / `budget_window` are re-read from the loop's
`ralph.toml` every tick, so editing it needs no restart. Spend is the sum of *every*
`ledger.jsonl` line inside the window — never deduplicated by iteration, because a LIMIT
retry appends a second line for the same iteration and that money was really spent. No
ledger or no `budget_usd` and the line is absent; enforcement is ralph's, this is only the
warning.

## Abnormal-exit posts, and `/msg`

A loop **ralphd itself spawned** that exits nonzero produces a post with the last `ABORTED`
line from `run.log` plus **Start again** / **Start on opus** (`--model opus` appended)
buttons, so a stuck loop is unblockable from a phone. Graceful exits get no post — `ralph`'s
own webhook announces those. A loop ralphd did *not* spawn (hand-launched, or from a
previous ralphd) is reparented to init and reaped there, so it has no exit status: it just
stops appearing as running, with no post and no buttons. That absence is expected, not a
bug.

`ralph msg` outlives Discord's 3s ack window, so ralphd **defers** the interaction, replaces
the placeholder at once, then edits one live message at ~20s and every 60s after with
elapsed, steps, tokens and current tool. At **14 minutes** it deletes the deferred reply and
migrates to a plain channel message, because Discord expires the interaction token at 15.
The result splits at line boundaries into at most **4** messages of 2000 chars, fences are
closed and reopened with their language tag across a split rather than torn, overflow past 4
is dropped with `… (N more message(s) omitted)`, and mentions are suppressed so a session
can never ping `@everyone`.

**Starting without a Discord round-trip:** `ralph start` in the repo writes
`<state_dir>/START`; ralphd polls every **3s**, consumes the marker whether or not it acts,
launches if nothing is running, and posts the new pid.

## Discord-side setup checklist

1. Create an application + bot at <https://discord.com/developers/applications>.
2. Invite with the **`bot`** *and* **`applications.commands`** scopes — without
   the latter the slash commands never register, silently.
3. Grant **Manage Messages** in each loop channel; that is what pinning needs.
4. Enable **no** privileged intents — ralphd connects with
   `GatewayIntents::empty()` and never reads message content.
5. Developer Mode → right-click → Copy ID: the guild, yourself, one channel per
   loop. Commands appear seconds after connect (guild-scoped registration skips
   the global propagation delay); if they don't, see the first trap.

## TRAPS

**Exactly ONE ralphd per guild.** `GuildId::set_commands` **replaces the guild's entire
command set**, so two instances silently unregister each other's commands — last to connect
wins, no error anywhere in either process. On connect ralphd logs what it is about to
overwrite (`replacing ALL N guild commands in <id> (…)`); unexpected names in that list mean
another instance is running. Many loops is what the config file is for; a second process is
not.

**`DISCORD_WEBHOOK` is env-only in `ralph`, and children inherit the environment.** One
ambient webhook would funnel every loop's lifecycle posts into a single channel, so ralphd
sets it per child and `env_remove`s it for a loop declaring none. Give every `[[loop]]` its
own `webhook`. Exact exception: a config with **exactly one** `[[loop]]`, and the flag form,
still inherit an ambient `DISCORD_WEBHOOK`; two or more loops never do. A webhook-less loop
logs `has no webhook — its lifecycle posts are off` and runs quiet.

**`ralph` owns `.ralph/loop.pid` — ralphd must never write it.** It only reads it (is this
loop running, refuse a duplicate `/start`, see a loop it did not launch) and clears it when
the recorded pid is dead. The one-loop-per-repo invariant is `ralph`'s startup guard;
duplicating it here is how it stops holding for hand-launched loops. Do not add a write
path.

**Spawned loops get their own session (`setsid` in `pre_exec`).** `ralph` fires
`kill -9 -<pgid>` sweeps at its iteration subtree between iterations; sharing a group, those
sweeps take ralphd down too — once observed as one `kill` SIGKILLing ralphd and both ralph
processes. Do not remove it.

## Debugging

| Symptom | First thing to check |
|---|---|
| Commands absent or vanished | Second ralphd in the guild; `applications.commands` scope; the `replacing ALL N` log line |
| `not authorized in this channel` | Wrong user id, or the channel has no `[[loop]]` |
| Reply is `rejected: …` | That is `ralph` refusing — reproduce the same `ralph` command in `dir` |
| No status card | Loop not actually running (`loop.pid`), or under 30s since start |
| Card not pinned | Missing Manage Messages — harmless, it still updates |
| Loop stopped, no exit post | ralphd did not spawn it, or it exited zero |
| `/msg`: "application did not respond" | Empty `message` option — ralphd returns before deferring |
| Config edit ignored | Only `ralph.toml` is re-read live; restart ralphd for `ralphd.toml` |

`cargo test --offline` in `tools/ralphd` covers the gateway-free core: config parsing, the
auth predicate, `poll_start`, `card_text`, the NDJSON fold, ledger summing, chunking.
