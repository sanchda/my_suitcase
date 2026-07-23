# Ralph prompt-authoring hints

Hard-won lessons for writing a per-project `PROMPT.md` (start from the
`ralph init` template) and configuring a healthy loop. The prompt is the stable
base fed *fresh* to every iteration; the runner appends the linted contract, the
resolved leaf, and your previous summary. Keep it short, project-specific, and
honest. See also `ralph schema` (backlog format), `ralph brief` (the resolved
iteration prompt), and `ralph lint` (validate the backlog).

## The prompt is a contract, not a tutorial
- Every iteration is fresh context with no memory except the runner's injected
  carry-forward. Assume nothing persists.
- Fill every `{{...}}` placeholder; delete guidance you don't need. Don't restate
  routing — the runner owns leaf selection. The prompt says how to *do* a leaf.
- Lean is cheap and reliable: omit hooks/plugins/MCP/memory and restrict tools to
  what the work needs (`extra_args = ["--safe-mode", "--tools", "Bash,Edit,Read,Write"]`).
  Fresh iterations gain nothing from session persistence.

## Verification is the whole game — make it exact and honest
- The most common failure is a confidently-wrong "done." Give the EXACT
  command(s) and the EXACT success marker (a passing line, a zero exit, a
  specific string). Vague "run the tests" invites faked green.
- Ask for the cheapest check that actually proves the increment, plus one final
  relevant check — not the whole suite every time, and never re-run unchanged
  green suites.
- Record KNOWN-BASELINE failures explicitly ("SuiteX has 2 pre-existing failures;
  do not attribute them to new work"), or the agent chases ghosts or fakes around
  them.
- Put "never claim a check you did not run" in every prompt.

## Any process the agent starts is the agent's to stop
This is the lesson that takes down machines.
- A game engine, dev server, browser, or headless harness left running leaks
  memory and OOMs the box. Prefer headless / one-shot invocations with a frame or
  time budget so the process EXITS ON ITS OWN. Avoid GUI/interactive sessions
  unless a screenshot is genuinely required.
- Never sleep-poll a process that is growing or wedged — kill it and move on.
  Watching RAM climb is not progress.
- Always tear down (`pkill`/quit the tool) before ending the turn.
- Harness backstop: ralph SIGKILLs the child's whole process group after every
  iteration, so a leak can't survive into the next step — but a *single*
  iteration can still balloon and OOM, so keep this guidance in the prompt AND set
  `iteration_timeout` (it also reaps a hung tool subtree).

## Commit discipline keeps history clean and recoverable
- One verified increment per commit; commit only after verification passes.
- Stage only the product paths changed this iteration, explicitly. Never
  `git add -A` or `git add .`. The `.ralph/` working set is untracked runtime
  state — never stage it.
- Concise imperative subject; state your trailer policy (e.g. no AI/co-author
  attribution).
- Forbid reset/rebase/amend/force-push/branch-switching and disturbing unrelated
  worktree changes.

## The end-of-turn summary is the only handoff
- The runner distills your final message into the next iteration's carry-forward.
  State: what changed, the exact proof you ran, and any constraint the next
  iteration must respect.
- Do NOT write PROGRESS or a `Next:` line — the runner owns the handoff and
  routing. Check off the finished task in the backlog in the same iteration.

## Right-size each leaf
- Work only the resolved leaf. If it won't fit one iteration, make a `plan` pass:
  split it into ordered child stages, each with an ID and a real `Verify:`
  contract, run `ralph lint`, and leave code for the first child.
- A good leaf names a bounded outcome and a runnable verification. See
  `ralph schema`.

## Let the loop pick the model; escalate, don't block
- A leaf's `(tier/…)` decoration sets its model automatically; a one-shot
  `.ralph/MODEL` overrides only the next pass.
- Needing a bigger model is NEVER `blocked` — escalate via decoration/override.
  Reserve `blocked` for a genuine human-only dead-end (approval gate, missing
  credentials/authority, the same failure after escalation). Consecutive blocked
  passes halt the loop.

## Set the safety knobs (`ralph.toml`)
- `iteration_timeout` — reap hung/heavy iterations and their whole tool subtree.
  Set it whenever the work launches engines, servers, or browsers.
- `max_cost_usd`, `max_duration`, `abort_after` — bound runaway spend, time, and
  no-progress streaks.
- `effort` / model tiers — match compute to the work.

---

Learned something new worth keeping? Add it here (`tools/ralph/HINTS.md`) so the
next project starts from the lesson instead of relearning it.
