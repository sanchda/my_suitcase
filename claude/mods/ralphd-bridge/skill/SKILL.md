---
name: ralphd-bridge
description: Use when controlling a ralph loop from Discord - launching or configuring ralphd (the single-tenant Discord control bridge), using its slash commands (/start /stop /model /status /next /backlog-add /backlog-edit /btw), or diagnosing refused commands, a missing status card, or missing lifecycle posts.
---

# Ralphd Bridge

`ralphd` is a thin, always-on, single-tenant Discord-to-CLI bridge for a
ralph loop. It never reimplements loop logic: the control plane is the
`.ralph/` files plus the `ralph` CLI. Separate binary (built by
`personalize/scripts/setup_ralph.sh`); ralph itself stays Discord-free.
Full usage: `ralphd --help`.

## Launch

Foreground, in the repo root (nohup/systemd for persistence):

    DISCORD_BOT_TOKEN=... ralphd \
      --guild <GUILD_ID> --channel <CHANNEL_ID> --user <USER_ID> \
      [--working-dir <repo>] [--autostart] -- <ralph args>

- Every setting takes a flag or an env var (flag wins); the token is
  env-only: `RALPHD_GUILD_ID`, `RALPHD_CHANNEL_ID`, `RALPHD_USER_ID`,
  `RALPHD_WORKING_DIR`, `RALPHD_AUTOSTART`.
- Everything after `--` is the fixed loop profile, forwarded verbatim to
  `ralph` on `/start`.
- Invite the bot with the `bot` + `applications.commands` scopes. Pinning
  the status card also needs the Manage Messages permission; without it
  the card degrades to an ordinary message.

## Authorization

Every interaction is refused (ephemerally) unless it comes from the one
configured guild, channel, AND user. Commands are registered guild-scoped
only. The reason is stated in the design: a Discord message can steer a
permission-skipping autonomous agent, so no unauthorized channel or user
may reach any command. The restart buttons pass the same gate.

## Commands

`/start [model]` (refused if `.ralph/loop.pid` is live), `/stop`
(graceful only -- writes `.ralph/STOP`), `/model <tier>` (one-shot
override for the next iteration), `/status`, `/next`, `/backlog-add`,
`/backlog-edit`, `/btw <message> [model]` (one-off yolo claude session;
output split at line boundaries across up to 4 messages, code fences
never torn, all mentions suppressed).

## Channel behavior

- One pinned live status card, edited in place every ~30s while a loop
  runs: run-state, iteration, pending count, current + upcoming leaves,
  the live in-iteration line. On loop end it gets a final past-tense
  edit; the next run replaces it -- exactly one card, never a pile.
- When a ralphd-spawned loop exits abnormally, ralphd posts the abort
  reason (from `run.log`) with Start-again / Start-on-opus buttons.

## The split that trips people up

Loop lifecycle and progress posts come from ralph's own `DISCORD_WEBHOOK`,
not from ralphd. Point the webhook at the same channel; ralphd handles
only inbound commands and their direct replies. A silent channel during a
healthy run means the webhook is unset, not that ralphd is broken.
