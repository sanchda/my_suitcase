# claude-top — design

**Status:** approved (design), pending implementation plan
**Date:** 2026-07-17

## Purpose

A `ctop`-style, live-refreshing, **read-only** terminal dashboard for the Claude Code
instances running on the local machine. It answers, at a glance:

1. Which `claude` instances are running.
2. Which account each is logged into.
3. Where each one lives — working directory, git branch / worktree, and tmux pane.
4. How much usage the current account has consumed, broken down **by model** and
   **by instance**.

Scope is deliberately bounded: **local machine only, read-only** (no killing,
attaching, or mutating sessions). Live view refreshes on a timer; `q` quits.

## Non-goals

- No interactivity beyond quit and a usage-window toggle (no select/kill/attach).
- No cross-machine aggregation.
- No dependency on a running daemon or background collector.
- Not a billing source of truth — dollar figures are local estimates.

## Form factor & technology

- **Live read-only TUI**, full-screen (alternate screen), timer-refreshed.
- **Rust binary** using **ratatui + crossterm**, following the existing
  `suitcase/plan-vim-gate` pattern (self-contained Cargo crate built and installed
  by a `personalize/scripts/` script).
- **Native usage parsing:** the binary reads Claude Code transcript JSONL directly.
  No `node`/`ccusage` subprocess per tick — this keeps overhead low and the tool
  self-contained. Dollar estimates come from a small embedded model-pricing table.

## Project shape (mirrors plan-vim-gate)

```
suitcase/claude-top/
  Cargo.toml            # ratatui, crossterm, serde, serde_json, anyhow; [profile.release] opt-level = "s"
  Cargo.lock
  .gitignore            # /target
  src/
    main.rs             # arg parse, terminal setup, event/refresh loop, key handling
    discover.rs         # find claude PIDs; per-PID session id, config dir, account
    tmux.rs             # pane_pid map + ppid-ancestor walk -> pane; dir
    git.rs              # branch + worktree detection for a directory
    usage.rs            # JSONL transcript parse -> tokens/cost by model & session
    pricing.rs          # embedded model -> $/Mtok table (input/output/cache-read/cache-write)
    ui.rs               # ratatui layout + rendering
  README.md
  docs/superpowers/specs/2026-07-17-claude-top-design.md
personalize/scripts/setup_claude_top.sh
```

Binary name: `claude-top`, installed to `~/.local/bin/claude-top` (ctop is the
container tool; user may alias separately).

## UI layout — two panels, one screen

```
 claude-top — Discord · david.kurchez@discordapp.com          [today]  q:quit  t:window
┌ Instances ───────────────────────────────────────────────────────────────┐
│ PID    tmux     dir              branch / worktree     model    session tok │
│ 1634   0:0.0    ~/suitcase       master                opus     820k         │
│ 8382   1:0.1    ~/discord        feat/x  (wt:x)         opus     1.1M         │
│ 17852  3:0.0    ~/dev            main                   sonnet   340k         │
└────────────────────────────────────────────────────────────────────────────┘
┌ Usage — current account, today ─────────────────────────────────────────────┐
│ by model     tokens (in/out/cache)              est. $                        │
│  opus        2.1M / 480k / 6.3M                  $12.40                        │
│  sonnet      0.4M / 90k  / 1.1M                  $1.90                         │
│ ─ by instance ─                                                               │
│  1634 suitcase   opus    820k    $4.10                                        │
│  8382 discord    opus    1.1M    $6.00                                        │
└────────────────────────────────────────────────────────────────────────────┘
```

- **Account (item #2) is folded in**, not a separate panel: the header shows the
  account `claude-top` itself resolves; each instance row carries its own account
  only when it diverges (i.e. the instance set `CLAUDE_CONFIG_DIR` to a different
  config). Normally all instances share one account, so a dedicated column is
  suppressed to save space and shown only on divergence.
- The `model` shown per instance is that session's most-recently-used model.

## Data model

```
Instance {
  pid: u32,
  ppid: u32,
  session_id: Option<String>,     // from CLAUDE_CODE_SESSION_ID env
  config_dir: PathBuf,            // CLAUDE_CONFIG_DIR env, else ~/.claude
  account: Option<String>,        // oauthAccount.emailAddress from that config
  tmux: Option<TmuxPane>,         // session:window.pane
  dir: Option<PathBuf>,           // process cwd (lsof) else pane_current_path
  branch: Option<String>,
  worktree: Option<String>,       // set when dir is a linked git worktree
  model: Option<String>,          // latest model from the session transcript
  session_tokens: u64,
}

ModelUsage  { model: String, input: u64, output: u64, cache_read: u64, cache_write: u64, cost_usd: Option<f64> }
InstanceUsage { pid: u32, label: String, model: String, tokens: u64, cost_usd: Option<f64> }
```

## Data flow (per refresh)

Two independent cadences so the cheap panel stays snappy:

### Instances — ~2s tick
1. `ps -axo pid,ppid,command`; keep processes whose command is the real `claude`
   CLI (argv0 `claude`, not the `bash -c` snapshot wrappers).
2. Per PID, `ps -Eww -p <pid>` → parse `CLAUDE_CODE_SESSION_ID` and
   `CLAUDE_CONFIG_DIR` from the environment.
3. Account = `oauthAccount.emailAddress` read from `<config>/.claude.json`
   (`~/.claude.json` for the default dir). Cached by config-dir path.
4. `tmux list-panes -a -F '#{pane_pid} #{session_name}:#{window_index}.#{pane_index} #{pane_current_path}'`
   → build `pane_pid -> pane`. Map each instance by walking its ppid ancestor
   chain until a pid matches a `pane_pid`.
5. Dir = process cwd via `lsof -p <pid> -a -d cwd -Fn`; fall back to
   `pane_current_path`.
6. Branch/worktree: `git -C <dir> rev-parse --abbrev-ref HEAD`; linked-worktree
   detected when `git -C <dir> rev-parse --git-common-dir` differs from
   `.git` under the toplevel.

### Usage — ~5s tick, incremental
1. Enumerate `<config>/projects/**/*.jsonl` for the current account's config dir.
2. Tail incrementally: track per-file `(len, mtime)`; only parse newly appended
   bytes, accumulating totals across ticks (full reparse only if a file shrank or
   its mtime is older than the tracked value — indicating rotation/rewrite).
3. For each assistant message line, read `message.usage`
   (`input_tokens`, `output_tokens`, `cache_creation_input_tokens`,
   `cache_read_input_tokens`) and `message.model`; aggregate **by model**.
4. Filename `<session-id>.jsonl` links a transcript to a running instance for the
   **by-instance** rollup and each instance's `model` / `session_tokens`. The
   by-instance rollup is always that live session's **cumulative** total (not
   window-filtered) — it reflects the running instance.
5. The window toggle applies to the **by-model** aggregate only: it filters each
   line by timestamp to today / last-7-days / all-time.
6. `$` via `pricing.rs`.

## Pricing

`pricing.rs` holds a static table: model id → USD per million tokens for input,
output, cache-read, and cache-write. Covers the current Claude family
(opus, sonnet, haiku, fable). Unknown model → tokens shown, cost rendered `$—`.
The table is a single well-commented const so it is trivial to update; a stale
table degrades to missing-cost, never a crash or wrong-crash.

## Defaults (configurable via keys / flags)

- Usage window (by-model panel only): **today**; `t` cycles today → last-7-days →
  all-time. The by-instance rollup is always the live session's cumulative total.
- Refresh: instances 2s, usage 5s.
- `q` quits.

## Error handling & degradation

Every external input is best-effort and isolated:

- No tmux / not in tmux → tmux column blank.
- Directory not a git repo → branch/worktree blank.
- `lsof` unavailable or denied → fall back to pane path.
- `ps -Eww` env not readable → session/config-dir unknown; account falls back to
  the default config's account.
- Unknown model in pricing table → tokens shown, `$—`.
- Malformed / partial JSONL line → skipped, not fatal.

The tool never aborts on a missing or oddly-shaped source. A one-line footer notes
any currently-degraded source (e.g. "tmux not found", "lsof denied").

## Testing

Logic lives in pure functions with fixture-based unit tests; the render/event loop
is a thin shell so nothing requires a live TTY:

- `usage.rs`: sample JSONL (multiple models, cache tokens, timestamps spanning the
  window boundary) → expected per-model and per-session totals; incremental-tail
  correctness (append then reparse only the delta).
- `pricing.rs`: known + unknown models → expected cost / `None`.
- `tmux.rs`: sample `list-panes` output + a ps ppid tree → expected instance→pane
  mapping, including multi-hop ancestor walk.
- `git.rs`: exercised against temp repos (plain repo, linked worktree, non-repo).
- `discover.rs`: env-string parsing (`CLAUDE_CODE_SESSION_ID`, `CLAUDE_CONFIG_DIR`)
  from sample `ps -Eww` lines; `claude` process filtering vs. wrapper noise.

## Install (personalize/scripts/setup_claude_top.sh)

Follows `setup_plan_vim_gate.sh`:

1. Resolve `SUITCASE_ROOT`; `PROJECT_DIR="$SUITCASE_ROOT/claude-top"`.
2. Require `cargo` (rustup); error clearly if absent.
3. `cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml"`.
4. Copy `target/release/claude-top` → `~/.local/bin/claude-top`.
5. No settings.json changes (this tool needs none) — unlike plan-vim-gate.

Runtime deps: `ps`, `tmux` (optional), `git` (optional), `lsof` (optional). All
degrade gracefully when absent.

## Open items / future (out of scope now)

- Interactive actions (jump-to-pane, kill) — explicitly deferred.
- Rate-limit bars — not available on enterprise accounts (no `rate_limits` in the
  JSON); would light up automatically only for Pro/Max, so not built now.
- Cross-machine aggregation.
```
