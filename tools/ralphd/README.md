# ralphd — Discord control bridge for ralph loops

An always-on foreground binary that lets one Discord user drive **one `ralph`
loop per channel** with native slash commands. The channel a command is typed in
is what selects the loop — `/status` in `#number-grove` means that repo. There is
no `--repo` argument.

ralphd shells out to the `ralph` CLI for everything and only *reads* `.ralph/`,
so it is **never load-bearing**: anything you can do from Discord you can do from
a terminal, and killing ralphd loses nothing. The loop runner itself is
documented in [`../ralph/README.md`](../ralph/README.md); this file is the
authoritative one for the bridge.

## Install

The suitcase personalize script builds ralphd in release mode and installs it to
`~/.local/bin/ralphd`:

```bash
$SUITCASE/personalize/scripts/setup_ralph.sh
```

Re-run it after source changes. `ralph` must also be on PATH — ralphd invokes it
by bare name.

## Auth model

Single-tenant by construction: **one guild, one user, N channels.** A command is
honored only when it arrives from the configured user *and* from a channel that a
configured `[[loop]]` claims. Anything else gets an ephemeral "not authorized in
this channel" and no action is taken. Button clicks pass the same gate.

That is the whole security model (`src/auth.rs`). There are no roles, no
allowlists, no per-command permissions — ralphd runs commands in your shell as
you, so treat the bot token and the user id as the only fence there is.

Successful replies are ordinary channel messages rather than ephemeral ones, on
purpose: the channel doubles as the audit trail for what was run.

## Config file

```toml
# ~/.config/ralphd.toml

guild = 123456789012345678        # the one guild;  or RALPHD_GUILD_ID
user  = 234567890123456789        # the one authorized user;  or RALPHD_USER_ID

[[loop]]
name      = "number-grove"                                    # card label
channel   = 345678901234567890                                # drives this loop
dir       = "/home/me/dev/number_grove"                       # ralph's cwd
args      = ["--model", "sonnet"]                             # optional
webhook   = "https://discord.com/api/webhooks/111/aaa"        # optional
autostart = false                                             # optional

[[loop]]
name      = "suitcase"
channel   = 456789012345678901
dir       = "/home/me/suitcase"
args      = ["--max-iterations", "40", "--dir", "/var/lib/ralph/suitcase"]
webhook   = "https://discord.com/api/webhooks/222/bbb"
autostart = true
```

```bash
DISCORD_BOT_TOKEN=… ralphd --config ~/.config/ralphd.toml
```

[`ralphd.toml.example`](ralphd.toml.example) is the same thing with every key
commented. Copy it rather than this block.

| Key | Required | What it does |
|---|---|---|
| `guild` | yes (or `RALPHD_GUILD_ID`) | The guild slash commands are registered in |
| `user` | yes (or `RALPHD_USER_ID`) | The only user whose commands are honored |
| `loop.name` | yes | Label on the status card and in ralphd's logs |
| `loop.channel` | yes | The channel that drives this loop; unique across loops |
| `loop.dir` | yes | Repo the loop runs in — `ralph`'s cwd |
| `loop.args` | no | Forwarded verbatim to `ralph` on `/start` |
| `loop.webhook` | no | Set as `DISCORD_WEBHOOK` on this loop's child |
| `loop.autostart` | no (default `false`) | Start this loop as soon as ralphd connects |

Unknown keys are a hard parse error (`deny_unknown_fields`), as is a config with
no `[[loop]]` at all or two loops claiming one channel.

A `--dir` or `--config` inside `args` also tells ralphd where that loop's state
and `ralph.toml` live, so the card reads the same files the loop writes; relative
paths resolve against `dir`, matching `ralph`. Every shelled-out command is aimed
there too, as `RALPH_DIR` / `RALPH_CONFIG` on the child — `ralph` honors those
flags from argv only for the loop itself, `stop` and `msg`, so the environment is
what makes `/status`, `/add` and the rest act on the relocated state dir. A
`--backlog` travels the same way as `RALPH_BACKLOG`, since `ralph` resolves the
backlog independently of the state dir. A loop whose `args` relocate nothing is
left alone, so a `dir` in its `ralph.toml` still decides.

The bot token is env-only and never appears in the file:

```bash
DISCORD_BOT_TOKEN=…      # required
```

### Single-loop flag form

The original flags still work unchanged as the degenerate case:

```bash
DISCORD_BOT_TOKEN=… ralphd \
  --guild <GUILD_ID> --channel <CHANNEL_ID> --user <USER_ID> \
  [--working-dir <repo>] [--autostart] -- <ralph args forwarded to /start>
```

Every flag also reads from an environment variable (the flag wins):
`RALPHD_CONFIG`, `RALPHD_GUILD_ID`, `RALPHD_CHANNEL_ID`, `RALPHD_USER_ID`,
`RALPHD_WORKING_DIR`, `RALPHD_AUTOSTART`.

**Precedence, first match wins:** explicit `--config` / `RALPHD_CONFIG` → the
flag form (whenever `--channel` or `RALPHD_CHANNEL_ID` is set) → `~/.config/ralphd.toml`
if it exists. The flag form beating the default path is deliberate: dropping a
config file on the box must not hijack a launch that already works.

Run `ralphd --help` for the same ground in one screen.

## Commands

Each acts on the loop that owns the channel you type it in.

| Command | Shells out to |
|---|---|
| `/start [model]` | `ralph <loop args> [--model …]` |
| `/stop [now]` | `ralph stop` / `ralph stop --now` |
| `/model <tier>` | `ralph model <tier>` |
| `/status`, `/next` | `ralph status --json` |
| `/add <title> [verify] [id] [under]` | `ralph add [--under P] [id] <title> [--verify …]` |
| `/drop <id> [recursive]` | `ralph drop <id> [--recursive]` |
| `/done <id>` | `ralph done <id>` |
| `/uncheck <id>` | `ralph uncheck <id>` |
| `/backlog-edit <id> <title> <verify>` | `ralph backlog edit --id … --title … --verify …` |
| `/msg <message> [model] [new]` | `ralph msg [--new] [--model …] --stream-json <text>` |

`/start`'s optional model is appended to the loop's `args`, so it wins over the
launch profile for that one run. Validation of tiers, ids and backlog schema all
live in `ralph` — ralphd echoes whatever it says back into the channel, including
rejections.

`ralph start`, run in a repo from a terminal, drops a `START` file in that loop's
state dir. ralphd polls for it every 3s and launches the loop, so a local process
can start a loop without any Discord round-trip.

## The pinned status card

While a loop runs, ralphd keeps **one pinned message per channel**, edited in
place every 30s: loop name and pid, run state, iteration, pending count, current
and upcoming leaves, the live in-iteration line from `.ralph/live`, spend from
`.ralph/ledger.jsonl`, and a relative "updated" stamp. The watcher sleeps before
its first poll, so the card appears up to 30s after the loop starts.

When the loop ends the card gets a final past-tense edit and stays as that run's
record. The next run deletes it (which unpins it) and pins a fresh one — one card
per channel, never a pile of status posts. That holds within one ralphd process:
the card's message id is only held in memory, so a restarted ralphd pins a fresh
card and leaves its predecessor's behind to be unpinned by hand. If pinning fails
the card degrades to an ordinary message and keeps updating.

**Budget warning at 80%.** Spend is the sum of *every* ledger line inside the
repo's `budget_window`, compared against the `budget_usd` in that repo's
`ralph.toml`. A LIMIT retry appends a second line for the same iteration and that
money was really spent, so lines are never deduplicated by iteration. Enforcement
is `ralph`'s; ralphd only surfaces the number. No ledger, or no configured
budget, and the line is simply absent.

## Abnormal-exit posts

When a loop that **ralphd itself spawned** exits nonzero, ralphd posts the abort
reason — the last `ABORTED` line in the trailing 16 KiB of `run.log` — with two
buttons:

- **Start again** — restart with the loop's configured args.
- **Start on opus** — restart with `--model opus` appended.

The "come look" signal carries its own remedies, so you can unblock a stuck loop
from your phone. Both buttons pass the same auth gate as commands. Graceful exits
get no post; `ralph` already announces those through its own webhook.

A loop started outside this ralphd process is reparented to init and reaped
there, so it has no exit status to report — it just stops appearing as running.

## `/msg` output hygiene

`/msg` drives a persistent claude session that far outlives Discord's 3s ack
window, so ralphd defers the interaction and keeps one live message current: the
placeholder is replaced immediately, the first progress line lands ~20s in, then
every minute with elapsed time, step count and tokens so far. Past 14 minutes the
live message migrates off the interaction token (Discord expires it at 15) into a
plain channel message that never does.

The result is split at line boundaries into at most 4 messages of 2000 chars, and
anything past the fourth is dropped with a `… (N more message(s) omitted)` note.
Code fences are closed and reopened with their language tag across a split rather
than torn, and all mentions are suppressed so a session cannot ping `@everyone`.

## Setup

1. Create an application and bot at <https://discord.com/developers/applications>.
2. Invite it with the **`bot`** and **`applications.commands`** scopes. Without
   `applications.commands` the slash commands never register.
3. Grant **Manage Messages** in the loop channels — that is what pinning the
   status card needs. Without it everything else still works and the card
   degrades to an unpinned message.
4. No privileged gateway intents are needed; ralphd connects with none. It never
   reads message content.
5. Collect the guild id, your own user id, and one channel id per loop
   (Developer Mode → right-click → Copy ID).
6. Copy `ralphd.toml.example` to `~/.config/ralphd.toml`, fill it in, and run:

   ```bash
   DISCORD_BOT_TOKEN=… ralphd --config ~/.config/ralphd.toml
   ```

Commands appear within seconds of connect — guild-scoped registration is not
subject to the global command propagation delay.

## Traps

Each of these cost real debugging time.

**Run exactly ONE ralphd per guild.** `GuildId::set_commands` *replaces the
guild's entire command set*. Two ralphd instances in one guild therefore silently
unregister each other's commands, last one to connect wins, with no error
anywhere. ralphd logs the full list it is about to overwrite on connect — if that
list contains commands you did not expect, another instance is running. Many
loops are what the config file is for; a second process is not.

**`DISCORD_WEBHOOK` is env-only in `ralph`, and children inherit it.** An ambient
webhook would therefore funnel every loop's lifecycle posts into whichever single
channel ralphd's own environment names. ralphd sets it explicitly on each spawned
child, and *clears* it for a loop that declares none rather than leaking its own.
The one exception is a config with exactly one loop, which still inherits an
ambient `DISCORD_WEBHOOK` so single-loop deployments behave as they always have.
Two or more loops never inherit it — give each `[[loop]]` its own `webhook`.

**ralphd does not write `.ralph/loop.pid` — `ralph` owns it.** ralphd only reads
it, to answer "is this channel's loop running", to refuse a duplicate `/start`,
and to see a loop it did not launch. A recorded pid that is dead is treated as
stale and the file removed. Do not add a write path here; the single-loop-per-repo
invariant is `ralph`'s startup guard, and duplicating it in ralphd is how it stops
holding for hand-launched loops.

**Spawned loops get their own session (`setsid`).** `ralph` fires
`kill -9 -<pgid>` sweeps against its iteration subtree between iterations. Shared
one process group with ralphd, those sweeps take ralphd down too — observed once,
as a single `kill` SIGKILLing ralphd and both ralph processes. See the comment in
`src/ralph.rs`; do not remove it.

## Development

```bash
cargo test --offline                    # unit tests, no network, no Discord
cargo clippy --all-targets --offline
```

The logic worth testing is factored to be pure and testable without a gateway:
config parsing (`config.rs`), the auth predicate (`auth.rs`), the START-trigger
decision (`handler.rs::poll_start`), card rendering (`card.rs::card_text`), the
NDJSON fold (`msg.rs::ingest`), ledger summing (`ledger.rs`), and message
chunking (`chunk.rs`).

| File | What lives there |
|---|---|
| `main.rs` | usage text, config load, client build, per-loop watchers |
| `config.rs` | flag/env/TOML parsing into one `LoopConfig` per channel |
| `auth.rs` | the whole auth predicate |
| `handler.rs` | command registration, dispatch, spawning, START watcher |
| `ralph.rs` | every shell-out to the `ralph` binary |
| `card.rs` | the pinned status card and its refresh loop |
| `ledger.rs` | spend and budget, read-only |
| `msg.rs` | driving the streaming `/msg` session |
| `chunk.rs` | fence-safe Discord message splitting |
| `format.rs` | `ralph status --json` → a Discord message |
| `loop_pid.rs` | read-only view of `ralph`'s pidfile |
