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

```bash
cargo build --release
cp target/release/plan-vim-gate ~/.local/bin/
```

Then add the hook to `~/.claude/settings.json` (merge into existing `hooks`):

```json
{
  "hooks": {
    "PermissionRequest": [
      {
        "matcher": "ExitPlanMode",
        "hooks": [
          { "type": "command", "command": "plan-vim-gate", "timeout": 345600 }
        ]
      }
    ]
  }
}
```

> Only one hook should own the `ExitPlanMode` gate. If plannotator is installed,
> disable its plan hook so the two don't collide.

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
