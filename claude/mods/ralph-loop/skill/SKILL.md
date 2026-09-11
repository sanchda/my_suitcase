---
name: ralph-loop
description: Use when setting up, driving, watching, or debugging a ralph autonomous loop, or when authoring or curating a .ralph/BACKLOG.md. Covers ralph init, backlog schema v1 essentials, lint/brief/status/backlog-edit, watching and stopping a running loop, and the gotchas (gitignored .ralph, completion marker discipline, blocked semantics).
---

# Ralph Loop

`ralph` runs `claude -p` in a loop, fresh context each iteration, until a
completion marker appears. Cross-iteration state lives in files, not
context. Authoritative references: `ralph schema` (backlog format),
`ralph --help`, and `$SUITCASE/tools/ralph/README.md`.

## Mental model

- The runner is global (`~/.local/bin/ralph`, built by
  `personalize/scripts/setup_ralph.sh`). Everything that drives a run is
  local to the repo: `.ralph/PROMPT.md`, `BACKLOG.md`, optional
  `VISION.md`, runner-owned `PROGRESS.md`, optional `ralph.toml`.
- The entire `.ralph/` working set is gitignored runtime state, never
  product. Product commits carry only code.
- Run one `ralph` per worktree; each loop drives the repo it launches in.

## Setup flow

1. `ralph init` -- scaffolds `.ralph/` and appends the gitignore block.
   Idempotent; never overwrites existing files.
2. Fill every `{{...}}` in `.ralph/PROMPT.md`: the goal, the exact
   verification contract, the commit contract, the branch.
3. Author `.ralph/BACKLOG.md` to schema v1 (below). Do not seed
   PROGRESS.md -- the runner owns it.
4. Check routing, then run on a dedicated branch:

       ralph schema     # full authoring reference
       ralph lint       # validates; shows the selected leaf
       ralph brief      # the bounded context the model will receive
       ralph --once     # single pass first
       ralph --max-iterations 30

## Backlog essentials (schema v1)

- Ordered Markdown checkboxes: `- [ ] **ID -- Title.**` with unique IDs and
  a non-placeholder `Verify:` command per pending task.
- Two-space-indented children are ordered stages; their IDs are prefixed
  `<parent-id>.`. A parent with pending children is a container whose own
  `Verify:` is the closure gate after the children finish.
- Selection: the first unchecked task with no unchecked descendants, in
  document order.
- Checkboxes route work -- keep task-looking examples inside fenced code
  blocks.

## Driving and watching a run

- `cat .ralph/live` -- live status of the active iteration.
- `tail -f .ralph/current.log` -- raw stream (includes thinking).
- `tail -f .ralph/run.log` -- high-level progress and perf lines.
- `touch .ralph/STOP` -- graceful stop after the current iteration;
  re-running `ralph` resumes (the iteration counter persists).
- `nohup setsid ralph ... &` -- detached overnight runs.
- `ralph status [--json]` -- backlog frontier snapshot.
- `ralph backlog add --title <t> --verify <cmd>` / `ralph backlog edit
  --id <id> ...` -- atomic, lint-guarded writes, safe against a running
  loop.

## Gotchas

- Never stage anything under `.ralph/` -- check-offs included.
- The completion marker (default `RALPH_COMPLETE`) must be emitted only
  when the whole goal is genuinely done and verified; completion archives
  the backlog and closes the arc.
- `blocked` in HANDOFF.json is reserved for dead-ends only a human can
  clear. Needing a bigger model is never `blocked` -- tier decorations or
  a one-shot `.ralph/MODEL` override handle that.
- No-progress detection escalates the model tier, then aborts; check
  `run.log` for the reason.
