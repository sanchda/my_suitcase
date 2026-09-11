---
name: multi-agent-coordination
description: Use when orchestrating two or more parallel agents that share a worktree or any shared mutable resource (docker daemon, live service stack, database, hardware device). Covers ownership maps, cooperative locking with with-lock, hub-and-spoke communication, report files, and the hazards of shared mutable state.
---

# Multi-Agent Coordination

Run N agents against one worktree and shared infrastructure without losing
work or corrupting measurements. Collisions are prevented structurally
(ownership), serialized cooperatively (the lock), and routed through one
point (the orchestrator).

## Before spawning anything

1. **Write the ownership map.** A table: file/area -> owning agent -> rule.
   Every agent's prompt carries an explicit OWN list. Shared files several
   agents must touch (a workspace manifest, a shared registry) are marked
   additive-only.

   **A conditional grant is still a grant.** "You own X, and you may also
   fix whatever the compiler points at" hands an agent every file
   containing the thing it is changing. If a peer owns one of those, the
   map is already broken and the collision is the orchestrator's, not the
   agent's. Enumerate the files or serialise the tasks.
2. **Name the contended resources.** A resource is anything mutable and
   shared: a docker daemon, a live stack, a database, a device, a port
   range. Give each a short name; that name is the `--resource` argument.
   Two are easy to miss: the version control index (concurrent commits),
   and the shared build unit -- one crate, module, or package that every
   agent's test command compiles, where a peer's half-written file blocks
   everyone despite disjoint file ownership.
3. **Define lock scope by what actually contends.** List the commands that
   touch each resource; everything else runs lock-free. State the no-lock
   set explicitly (file edits, pure builds and unit tests) -- that is what
   keeps N-1 agents busy while one holds the lock.
4. **Render the prompt template.** Fill the placeholders in
   `prompt-template.md` (in this skill directory) and append it to every
   spawned agent's prompt.

## The lock

`with-lock` (suitcase `bin/`, on PATH) serializes access to a named
resource:

    with-lock --resource <name> --label <agent> [--timeout SECS] -- cmd ...
    with-lock --resource <name> --status
    with-lock --list

- Crash-safe: a kernel flock on a file descriptor, released when the holder
  dies, even on SIGKILL. A stale `.holder` file over a free lock is
  harmless; `--status` trusts the lock, not the file.
- Cooperative: nothing enforces it. The discipline lives in the prompts --
  that is why every agent's prompt carries the rules, not just the
  orchestrator's.
- Exit 75 means timed out: wait and retry. Never bypass.
- Held for exactly the wrapped command's lifetime; wrap a script for a
  multi-command critical section.

Norms that make it work:

- Hold for the shortest window that keeps a measurement coherent. Chunk
  long campaigns into bounded sessions; batch expensive resource cycles.
- Earn the lock cheaply, spend it expensively: have agents verify their
  logic offline before burning a lock window on a crash.
- **Release protocol.** Mutual exclusion is not enough for a mutable
  resource. After destructive or fault-injecting work, restore the resource
  to health (restart what the faults stopped, run the relevant GC) before
  the lock releases, so the next holder inherits a working state. Put the
  concrete restore steps in the agent's prompt.

## Communication

**Hub-and-spoke only.** Agents report to the orchestrator; the orchestrator
routes. Never peer-to-peer. Two reasons: the orchestrator holds the global
picture while each agent holds deep local context, and routing through one
point means no agent can be talked into editing a file it does not own.

**Resume, don't respawn.** Messaging a finished agent wakes it with its
full context intact. Use that for follow-up work instead of spawning a
fresh agent that must re-read the world.

**Report files are the durable half of the bus.** Each agent writes
`reports/<date>-<label>.md` with a mandated section: "Changes needed in
files you do not own". Peers implement from the report, not from relayed
chat. Reports survive the session; messages do not.

**Shared artifacts are the third channel.** Per-agent output directories
for raw captures; checked-in machine-readable contracts (manifests,
schemas) as interfaces. Peers read each other's artifacts read-only, with
no coordination -- that is the channel working as intended.

## Hazards of shared mutable state

- **Generation skew.** Mutual exclusion is not stability: another agent may
  legitimately cycle the resource between measurement arms. Record the
  resource generation (restart count, container IDs, a version stamp)
  before each arm and re-baseline when it changes.
- **Pin skew.** Pinning copies of shared tools buys reproducibility and
  costs currency with the tree: a pinned tool can report defects the tree
  already fixed. State both directions in the agent's prompt.
- **Edit conflicts.** When an Edit fails because a peer changed the file:
  re-read and re-apply. Never overwrite.
