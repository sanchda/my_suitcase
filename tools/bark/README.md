bark
====

Post an ID-tagged line to a Discord webhook. One process, one message, an exit
code that says whether it landed.

```sh
bark deploy-42 'gateway rollout finished'          # -> `[deploy-42]` gateway rollout finished
bark --to alerts oncall 'disk 91% on relay-7'
make build 2>&1 | tail -5 | bark build-$(git rev-parse --short HEAD) -
```

Every message carries an id, so a channel full of machine chatter stays
greppable and correlatable with whatever emitted it (build number, host, loop
iteration, ticket).

`bark --help` is the full reference.

Install
-------

```sh
~/my_suitcase/personalize/scripts/setup_bark.sh
```

Builds release, installs `~/.local/bin/bark`, and seeds a config if none exists.
Safe to re-run; never overwrites an existing config.

Config
------

First path that is set wins:

1. `--config <file>`
2. `$BARK_CONFIG`
3. `$XDG_CONFIG_HOME/bark/config.toml`
4. `~/.config/bark/config.toml`

```toml
webhook  = "https://discord.com/api/webhooks/<id>/<token>"   # single target
username = "bark"                                           # optional

default  = "ops"                                            # if several targets
[targets.ops]
webhook  = "https://discord.com/api/webhooks/<id>/<token>"
[targets.alerts]
webhook  = "https://discord.com/api/webhooks/<id>/<token>"
username = "pager"
```

- `bark init --webhook <url>` writes that file (mode 0600 -- it holds a token).
- `bark targets` lists what is configured, tokens redacted, default marked `*`.
- Webhook precedence: `--webhook`, `--to <name>`, `$BARK_WEBHOOK`, config default.
- Unknown keys are an error, so `webook =` fails loudly instead of posting nowhere.
- A single target needs no `default =` line; two or more without one is an error
  naming both.
- No usable config? The error names the path it checked and how to fix it:

```
$ bark deploy-1 'hi'
bark: no webhook configured: /home/dave/.config/bark/config.toml (file not found)
       fix: bark init --webhook <url>, or set $BARK_WEBHOOK, or pass --webhook <url>
```

Behavior worth knowing
----------------------

- Exit codes: `0` sent, `1` not delivered, `2` bad usage or config.
- Silent on success. `--wait` prints the created message id (useful for editing
  the message later); errors go to stderr.
- 429 and 5xx are retried up to 3 attempts, honoring Discord's `retry_after`
  (capped at 30s). A 4xx is not retried.
- Content is clamped to Discord's 2000 characters, marked ` [truncated]`.
- Mentions never resolve (`allowed_mentions.parse = []`), so a log line
  containing `@everyone` pings nobody.
- The POST shells out to `curl --config -`, so the webhook token arrives on stdin
  and never appears in `ps`. No HTTP crate, no TLS stack to keep patched.
- `-` as the entire message reads stdin; empty stdin is an error, not an empty
  post. Use `--` before a message starting with a dash.

Claude Code notifications
-------------------------

```sh
cc-mod enable bark-notify      # off again: cc-mod disable bark-notify
```

`bark-notify` is a cc-mod (`claude/mods/bark-notify`, enabled by default during
`personalize`). It adds three hooks to `~/.claude/settings.json`, all pointing at
`tools/bark/hooks/claude-bark.sh`:

| Event | Bark |
|---|---|
| `Notification` | `needs you: <what Claude is waiting for>` |
| `Stop` | `done: <last thing Claude said, 240 chars>` |
| `SessionEnd` | `session ended (<reason>)` |

Ids look like `claude/<dir>/<session prefix>`, so parallel sessions stay apart.
The hook needs `jq`, always exits 0, and posts with a 5s timeout.

Per-machine knobs: `BARK_CC_EVENTS` (default `Notification,Stop,SessionEnd`;
`SubagentStop` is also understood), `BARK_CC_TO` (target name), `BARK_BIN`.
`cc-mod disable bark-notify` turns it off and keeps it off (see
`claude/mods/README.md`).

A hook cannot show you its own errors, so failed posts (rotated webhook, no
network) land in `~/.cache/bark/claude-hook.log` (`BARK_CC_LOG`). If the
notifications go quiet, look there first.

Test
----

```sh
cargo test                  # unit + CLI tests; nothing touches the network
bark --dry-run id 'text'    # show the resolved target and rendered content
```
