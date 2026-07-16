# plan-vim-gate

A terminal-native `ExitPlanMode` gate for Claude Code. When Claude finishes
planning, the plan opens in **nvim in a tmux split**. You edit it, save, and
close the pane — and Claude proceeds with *your* edited plan. No browser.

## How it works

Claude Code fires a `PermissionRequest` hook on `ExitPlanMode`. This binary is
that hook: it reads the plan from stdin, opens it in an editor pane, blocks
until you close the pane, then emits an allow/deny decision from what you wrote.

```
Claude calls ExitPlanMode
  → plan-vim-gate reads .tool_input.plan from stdin
  → writes plan + directive header to a temp .md
  → nvim opens in a tmux split; the hook BLOCKS
  → you edit, save & CLOSE the pane
  → decision emitted from the buffer
```

## The buffer

```markdown
<!-- plan-gate
decision: approve
-->

<Claude's plan — edit freely>
```

- Leave `decision: approve`, edit the plan, save & close → your edited body
  becomes the plan Claude implements.
- Set `decision: reject`, replace the body with your notes → Claude revises and
  resubmits.
- Missing/garbled header, or an empty approved buffer → **reject** (fail-safe;
  never auto-approves by accident).

## Install

This project is part of the [suitcase](../Readme.md). Its personalize script
builds the binary, installs it to `~/.local/bin`, and wires the hook into
`~/.claude/settings.json` (idempotent, preserves other settings):

```bash
suitcase/personalize/scripts/setup_plan_vim_gate.sh
# or, with everything else: suitcase/personalize/personalize
```

Rebuild after source changes by re-running that script.

<details>
<summary>What it wires into <code>~/.claude/settings.json</code></summary>

```json
{ "hooks": { "PermissionRequest": [
  { "matcher": "ExitPlanMode",
    "hooks": [{ "type": "command",
                "command": "<HOME>/.local/bin/plan-vim-gate",
                "timeout": 345600 }] } ] } }
```

Only one hook may own the `ExitPlanMode` gate; the script replaces any existing
`ExitPlanMode` matcher with this one.
</details>

## Requirements

- Run Claude Code **inside tmux** (the gate needs a real pane; `$TMUX` must be
  set). Outside tmux the hook errors and Claude falls back to its built-in
  dialog.
- `nvim` and `tmux` on `PATH`.

## Config

| Env var | Default | Effect |
|---|---|---|
| `PLAN_GATE_APPROVE_MODE` | `input` | `input` → `allow` + edited `updatedInput.plan` (clean proceed; Claude implements your edited plan — verified). `deny` → `deny` + verbatim message (re-triggers the gate; fallback only). |
| `PLAN_GATE_EDITOR` | `nvim` | Editor command opened in the tmux split. |

### On prompt-injection resistance

Approve works by replacing the plan in `updatedInput.plan` with your edited
body — verified to reach the model as the approved plan. One caveat: an edit
that *flatly contradicts the whole conversation* can read to Claude like a
prompt-injection attempt, and it may disregard it. Normal plan edits
(refinements, reordering, added constraints, cuts) are fine. If you hit that
edge on a heavy rewrite, switch to the framed-feedback fallback:

```bash
export PLAN_GATE_APPROVE_MODE=deny
```

## Development

```bash
cargo test          # buffer parsing + decision JSON + shell-quote
cargo build --release
```

Modules: `hook_io` (stdin event) · `buffer` (header write/parse) ·
`editor` (tmux/nvim, blocking) · `decision` (decision JSON). See
`docs/superpowers/specs/` for the design.
