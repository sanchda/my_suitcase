# claude-top

A live, read-only TUI dashboard for the Claude Code instances running on this
machine. Two panels:

- **Instances** — each running `claude` process with its tmux pane, working
  directory, git branch / worktree, and account.
- **Usage** — current-account token/cost usage, broken down by model (windowed:
  today / 7d / all) and by running instance.

Usage is parsed natively from `~/.claude/projects/**/*.jsonl` — no ccusage/node
dependency. Dollar figures are local estimates from an embedded pricing table.

## Build & install

Via the suitcase personalize script:

    personalize/scripts/setup_claude_top.sh

or directly:

    cargo build --release --manifest-path claude-top/Cargo.toml
    cp claude-top/target/release/claude-top ~/.local/bin/

## Keys

- `q` / `Esc` — quit
- `t` — cycle usage window (today → 7d → all)

## Runtime dependencies

`ps` (required). `tmux`, `git`, `lsof` are optional — missing ones just blank
their columns. Rate-limit bars are intentionally absent: the `rate_limits` field
only appears for Claude Pro/Max, not enterprise/Team accounts.
