# vim-plan-gate — design

**Date:** 2026-07-15
**Status:** approved, implementing

## Problem

Claude Code's `ExitPlanMode` gate is browser-based (via plannotator) or the
built-in TUI dialog. Neither lets a heavy tmux + nvim user *edit* the plan in
their own editor and hand the edited version back as the authoritative plan.
We want a terminal-native plan gate: when Claude finishes planning, the plan
opens in nvim in a tmux split; you edit it, save & close, and Claude proceeds
with your edited plan.

## Mechanism (how the gate works)

Claude Code fires a `PermissionRequest` hook matching `ExitPlanMode`. The hook
is just a **blocking process**: it reads the hook event JSON from stdin
(`.tool_input.plan`), and whatever it prints to stdout becomes the decision.
The `hooks.json` timeout is set very high (`345600`s / 4 days), so the process
can block for as long as the user takes to edit.

```
Claude calls ExitPlanMode
  → hook fires, reads event JSON from stdin, extracts .tool_input.plan
  → writes plan + directive header to a temp .md
  → opens it in nvim in a tmux split; BLOCKS until the pane's editor exits
  → reads the file back, parses the header directive
  → emits decision JSON to stdout
```

**Release signal = editor close, not file save.** You save many times while
editing; you close once. Blocking uses tmux's own lock channel
(`wait-for -L/-U`), which is state-based and race-free — not fnotify-on-save
(wrong granularity) and not polling.

## Edit semantics: "rewrite the plan itself"

The edited buffer **becomes** the plan Claude implements — the user is the
final author, no re-plan round-trip.

**Mechanism (empirically verified in a live interactive Claude Code session,
2.1.210):** on an `ExitPlanMode` *approval*, a modified `updatedInput.plan`
**is** injected into the model's context as the approved plan — Claude sees and
proceeds from the edited text, not the plan it originally wrote. Two things
were ruled out along the way:

- `additionalContext` is **not** a supported field for `PermissionRequest`
  (docs + behavior); it is silently ignored on `allow`. Do not use it here.
- `deny` + `message` does reach the model, but keeps Claude in plan mode → it
  resubmits and re-triggers the gate (a loop). Kept only as a fallback.

**Decision: approve uses `allow` + edited `updatedInput.plan`** — clean
proceed, no loop, edits honored. A `deny + message` fallback (edited plan
framed "implement verbatim") is selectable via `PLAN_GATE_APPROVE_MODE=deny`
(no recompile) for the rare case a heavy rewrite trips Claude's
prompt-injection defense on the `allow` path (a plan that flatly contradicts
the conversation can read as an injection attempt; normal edits do not).

## Buffer contract

The hook writes a directive header above the plan:

```markdown
<!-- plan-gate
decision: approve

Edit the plan below, then save & CLOSE this pane to submit it as the final plan.
To reject: set `decision: reject` above and replace the body with your revision notes.
(Unknown keys are ignored — safe to extend.)
-->

<Claude's original plan markdown>
```

- The header is a small `key: value` block, chosen for legibility so other
  tools can read/write the decision and compose analysis later. Only
  `decision` is required; unknown keys are ignored (forward-compatible).
- The **body** (everything below the header) is always the payload:
  on approve it's the finalized plan; on reject it's the revision notes.

## Decision mapping

| Buffer state | Emitted decision |
|---|---|
| `decision: approve`, non-empty body | `allow` + `updatedInput.plan` = edited body (original plan replaced). Or, in `deny` mode, `deny` + `message` framed "finalized, implement verbatim". |
| `decision: approve`, empty body | Fail-safe → `deny` (reject) "empty plan buffer, re-plan". |
| `decision: reject` (or unrecognized / missing header) | `deny` + `message` = body notes (or generic "revise and resubmit"). |

**Fail-safe rule:** anything ambiguous (missing/garbled header, unknown
decision value, empty approve) resolves to reject. The gate never
auto-approves by accident.

## Architecture

Rust binary crate `plan-vim-gate` (chosen over a bash script for typed,
testable, extensible modules). Minimal deps: `serde`, `serde_json`,
`tempfile`.

| Module | Responsibility | Depends on |
|---|---|---|
| `hook_io` | Read/parse the ExitPlanMode event from stdin; hold `tool_input` + `plan`. | serde_json, stdin |
| `buffer` | Write the header + plan to a scratch `.md`; parse the header back; split off the body. | tempfile |
| `editor` | Spawn nvim in a tmux split, block via `tmux wait-for -L/-U`, error if not in tmux. | tmux, `$TMUX` |
| `decision` | Map the parsed outcome → decision JSON; emit to stdout; read `PLAN_GATE_APPROVE_MODE`. | serde_json, stdout |
| `main` | Orchestrate: read → write scratch → edit → parse → emit. | all |

## Data flow

```
stdin JSON ──hook_io──▶ {tool_input, plan}
   plan ──buffer.write_scratch──▶ temp.md (header+plan)
   temp.md ──editor.open_and_wait──▶ (blocks; user edits in nvim/tmux)
   temp.md ──buffer.parse──▶ {decision, body}
   {decision, body, tool_input} ──decision.emit──▶ stdout JSON
```

## Error handling / edge cases

- **No plan in event** → error exit (nothing to gate).
- **Not in tmux (`$TMUX` unset)** → error exit; Claude falls back to its own
  ExitPlanMode dialog. (Foreground-spawning an editor is not viable: the hook's
  stdio is pipes with no controlling TTY — that's precisely why the tmux split
  is required.) A new-terminal fallback is out of scope for v1.
- **split-window fails** → release the lock, error exit → Claude's own dialog.
- **Editor exits abnormally** (`:cq`, crash) → still releases the lock (`; ...`
  after nvim), so the hook doesn't hang; body is parsed as-is.
- **Missing/garbled header** → fail-safe reject.
- **Concurrency** → lock channel is namespaced by PID, so overlapping gates
  don't collide.

## Testing

- Unit tests in `buffer` for `parse`: approve/reject/unknown/missing-header/
  empty-body → correct `Decision` + body split.
- `cargo build --release`; install to `~/.local/bin/plan-vim-gate`.
- Injection mechanism (`allow` + edited `updatedInput.plan`) verified in a live
  interactive session via a canary hook: Claude read and reacted to the swapped
  plan. Normal edits proceed cleanly; only if a rewrite that contradicts the
  conversation trips the injection defense → `PLAN_GATE_APPROVE_MODE=deny`.

## Out of scope (v1 / YAGNI)

- Non-tmux / new-terminal spawning.
- The reject-loop UX beyond a single deny message.
- Reusing plannotator's server, history save, or sharing.
- Multi-editor abstraction (nvim assumed; `$EDITOR` only conceptual).
