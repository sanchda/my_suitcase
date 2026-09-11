---
name: ralph-kanban
description: Use when working a ralph backlog in parallel with subagents in the same worktree/repo (no new worktrees) - turning the serial ralph loop into an orchestrator-driven kanban board. Composes .ralph/BACKLOG.md task structure with the multi-agent-coordination harness (with-lock, ownership, hub-and-spoke).
---

# Ralph Kanban

Work a ralph backlog in parallel: the orchestrating session replaces the
serial ralph runner and dispatches waves of subagents in the one worktree.
The `.ralph/` structure stays the source of truth and must stay
schema-valid at all times. The `ralph` binary is used for `lint`,
`status --json`, `brief`, and `backlog add/edit`; no serial loop runs.

Invoke the `multi-agent-coordination` skill alongside this one -- its
rules (ownership, locking, hub-and-spoke, report files) apply verbatim
and are not restated here.

## Hard preconditions

- **Mutual exclusion with the serial runner.** Check whether a loop is
  live (`.ralph/loop.pid`, `cat .ralph/live`). If so: `touch .ralph/STOP`,
  wait for the iteration to end, then begin. Never launch `ralph` (or let
  a ralphd `/start` land) while a kanban wave is in flight.
- `.ralph/BACKLOG.md` exists and `ralph lint` passes.
- The `multi-agent-coordination` skill is installed (`with-lock` on PATH).

## Fixed policies

- **Orchestrator assigns.** The orchestrator computes the frontier and
  hands each agent exactly one task. Agents never scan the backlog and
  never claim tasks. BACKLOG.md has a single writer: the orchestrator.
- **Agents commit under lock.** One verified increment per commit, by the
  agent that did the work: name only owned files explicitly (never
  `git add -A` or `git add .`), wrap every git write in
  `with-lock --resource git -- git ...`, and follow ralph's safety rules
  (no reset, rebase, amend, force-push, or branch switch; never stage
  anything under `.ralph/`).

  **Staging and committing must be one locked operation.** This is the
  part that looks right and is not:

      # WRONG -- two lock acquisitions, and the index is shared state
      with-lock ... -- git add src/mine.rs
      with-lock ... -- git commit -m "..."

  Between those two commands the lock is released, so a peer can stage its
  own files into the same index; the bare `git commit` then commits
  *everything staged*, sweeping a peer's unrelated work into your commit.
  Nothing is lost, but commit boundaries stop meaning anything and the
  ownership map is silently violated by a well-behaved agent. Use a
  pathspec commit, which ignores index state entirely:

      with-lock ... -- git commit --only src/mine.rs reports/mine.md -m "..."

  One wrinkle: `--only` cannot name an **untracked** file (`pathspec did
  not match any file(s) known to git`), which bites on a new report. Keep
  both commands inside one acquisition rather than splitting them again:

      with-lock ... -- sh -c 'git add --intent-to-add reports/mine.md &&
        git commit --only src/mine.rs reports/mine.md -m "..."'

  The general rule: **a lock protects a critical section, not a command**
  -- if two commands must not interleave, they belong inside one
  acquisition. That is the invariant to preserve; the exact command shape
  is negotiable. `agent-commit` in the suitcase does all of the above in
  one call, which is worth preferring: a protocol that each agent must
  re-derive correctly will be got wrong by some of them, however clearly
  it is written.

  **There are two races here and git only closes one.** The above fixes
  the *index* race -- a peer staging a different file into the shared
  index while your lock is released. It does **not** fix the
  *working-tree* race: `--only <path>` commits that path's entire working
  tree, so a peer's uncommitted edits in a file you also touch land in
  your commit. No git primitive fixes that short of interactive staging.

  So it is prevented by ownership, not tooling, and that makes it the
  **orchestrator's** job: never let two agents hold write access to the
  same file at the same time -- including conditionally. "You own
  `gcdelta.rs`, and you may also fix the struct literals the compiler
  finds" is a grant over every file containing that literal, and if
  another agent owns one of them you have just created the race yourself.
  Enumerate the second agent's files explicitly, or serialise the two
  tasks. Agents can add a tripwire (`git status --porcelain -- <path>`
  before touching a file they did not write) but cannot distinguish their
  own edits from a peer's, so it catches only the case where they touch a
  file already dirty.

## The wave loop

1. **Frontier.** Read BACKLOG.md. Eligible = unchecked leaves with no
   unchecked descendants. Parallelize across top-level tasks only -- a
   parent's children are ordered stages by schema semantics and stay
   serial. Judge independence by expected file footprint, shared mutable
   resources, and **the shared build unit** (see hazards); size the wave
   accordingly. A wave of 1 is legitimate.

   **Dispatch as dependencies clear, not at a wave barrier.** "Wave" names
   the batch, not a synchronization point. A task whose inputs are already
   committed goes out immediately, even while the previous batch runs --
   waiting for every agent to finish before starting the next task wastes
   the parallelism the harness exists to buy. Barrier only where a later
   task genuinely needs all of an earlier batch at once.
2. **Dispatch.** One task per agent, same worktree. Each agent prompt
   contains:
   - a bounded brief: that task's ID, title, body, and `Verify:` contract
     only -- never the whole backlog (mirror `ralph brief` discipline);
   - the rendered `multi-agent-coordination` prompt template:
     `{{LABEL}}` = task ID, `{{OWN}}` = expected file footprint,
     `{{RESOURCE}}`/`{{RESTORE}}` = `git` plus any named project
     resources, `{{REPORT_PATH}}` = `reports/<date>-<task-id>.md`;
   - the commit contract above: verify first, then commit under the git
     lock;
   - the build-coupling rule: *a compile or build error in a file you do
     not own is a peer's in-flight edit, not your bug -- wait and retry,
     never fix it, never work around it, never revert it.* Without this an
     agent "helpfully" repairs a peer's half-written file and the
     ownership map is breached by a well-intentioned agent.

   **Label hypotheses as hypotheses.** An orchestrator relaying a peer's
   finding is the most likely place in this whole harness for an untested
   causal claim to harden into a stated fact: a report says "plausibly X",
   the brief says "Cause: X", and the agent implements against X. Write
   "peer reports X, unverified -- test it before acting" and say what
   would discriminate. An agent handed a confident wrong cause can waste
   an entire task on it, and the ones that push back are exercising
   judgement the brief should not have required of them.
3. **Collect.** Require each report to quote the exact verify command and
   its output.

   **A minor item bundled with a major one does not get done.** Attaching
   "and also tidy these comments" to a brief whose headline is a
   correctness bug reliably produces the correctness fix and nothing
   else -- the agent is right to prioritise, and the small item vanishes
   silently because nobody checks. One project applied about a dozen
   audit findings and left another dozen, named verbatim in the same
   report, untouched for two rounds; the orchestrator assumed the round
   was complete because the headline items were. Either dispatch the
   small work as its own task, or verify it specifically rather than
   inferring it from a green suite. The orchestrator checks off completed tasks in BACKLOG.md,
   runs `ralph lint` after every write, and records a wave summary in
   PROGRESS.md (orchestrator-owned in this mode, as the runner would).

   Expect `ralph lint` to warn once tasks complete out of document order
   (`checked task N appears after pending sibling M; document order was
   bypassed`). That is a warning, not an error: the schema stays valid and
   selection still picks the first pending leaf, so a handback to the
   serial runner is safe. Do **not** reorder the backlog to silence it --
   document order records the intended sequence, and out-of-order
   completion is the whole point of working the frontier in parallel.
4. **Failures.** A task that fails its verify stays unchecked. Choose:
   resume the same agent with routing guidance (resume, do not respawn);
   split the task into staged children via `ralph backlog add/edit`; or
   pull it out of parallel and run it as a wave of 1.
5. **Closure.** A parent whose children are all checked becomes a serial
   integration task in a later wave. When no pending leaves remain, run
   the final closure/verification serially. Arc archiving is manual or
   via handing the repo back to the serial runner -- the backlog never
   left schema, so the handback is safe by construction.

## Hazard: file ownership is not build isolation

Disjoint file ownership does not make agents independent when they share a
build unit. In a single-crate Rust project (or one Go module, one Python
package, one tsconfig project) every agent's test command compiles the
whole unit, so one agent's transiently uncompilable file blocks every
other agent's verification. Nobody violates the ownership map and the
whole wave stalls anyway: the build unit is a shared mutable resource the
map does not model.

Three mitigations, in order of preference:

- Carry the build-coupling rule in every agent prompt (above). The
  stall is tolerable; an agent editing a peer's file to clear it is not.
- Prefer a multi-crate workspace, or per-package tasks, when the project
  shape allows it -- that converts the shared build unit into per-agent
  ones and makes the isolation real rather than conventional.
- Have the orchestrator land a skeleton that compiles before dispatching,
  so agents only ever move the tree from green to green and the broken
  window is one agent's edit rather than a missing module.

Expect the stall to be reported by more than one agent; treat the first
report as information, not as a defect to chase.

**Watch for tools that look scoped and are not.** An agent obeying its
ownership map perfectly can still write to a peer's file through a
command that takes a path and ignores it. `cargo fmt -- src/mine.rs`
formats the *whole crate*; one agent had it silently reformat a peer's
file twice before noticing (`rustfmt src/mine.rs` is the scoped form).
The same trap exists in codemod tools, linters with `--fix`, and any
formatter configured at project rather than file level. Tell agents to
check `git status` after running a whole-project tool, and to revert
rather than commit anything outside their own paths.

## Verification: prove it, do not observe its absence

Ask agents to prove a fix by breaking it -- reintroduce the defect, watch
the specific test fail, revert, and report both. "The suite is green" is
not evidence that a new guard works; it is evidence that nothing else
broke. This single instruction produced more real findings than any
other in practice: agents discovered their own tests passing under the
very defect they targeted, and threw those tests away rather than ship
them as proof.

**The blind spot they share: a test that moves with the code it tests
proves nothing.** This showed up three times in one project, wearing a
different disguise each time, and a clean mutation run hid all three:

- **The assertion is implied by the code.** A population guard asserted
  `count <= live + slack`, while an early exit -- added in the *same
  commit* -- guaranteed `live >= count`. The guard was a tautology, and
  no single mutation could separate it from the thing that neutralised
  it. When one change adds both a check and a shortcut past it, require
  a test that fails if the shortcut is reintroduced.
- **The expectation is a copy of the source.** Four error strings were
  pinned by pasting them into a test, documented as "copied verbatim".
  Two no longer existed, the match arm consuming one was dead, and each
  kept the other looking covered. A copy asserts only that the test
  agrees with itself.
- **The expectation is the same constant.** A test compared a message
  against the constant that generates it, so gutting the constant left
  the test green. Shared-constant assertions catch typos and nothing
  else.

The rule that covers all three: **an assertion must reference a source
of truth independent of the code under test** -- the value the code
actually produced at runtime, a fact from outside the system, or a
property the code cannot satisfy trivially. When that is impossible, say
so where the test is, so the next reader knows its wording rests on
review rather than on the suite.

## Judgment notes

- Prefer small waves of clearly-independent tasks over large waves that
  need heavy locking. The lock serializes anyway; parallelism only pays
  when footprints are disjoint.
- When two eligible tasks touch the same file, either serialize them or
  put the file in `{{SHARED_ADDITIVE}}` for both -- never split ownership
  of one file across two writers.
- Verify claims are trusted only when the report quotes the command and
  output; spot-check by re-running a task's `Verify:` before checking off
  anything load-bearing.
