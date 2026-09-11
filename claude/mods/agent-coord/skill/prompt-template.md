# Per-agent coordination boilerplate

Render this block, placeholders filled, and append it to every spawned
agent's prompt. Placeholders:

- `{{LABEL}}` -- the agent's short name (also its lock label).
- `{{OWN}}` -- files/areas this agent owns outright.
- `{{SHARED_ADDITIVE}}` -- shared files it may append to, never rewrite.
- `{{RESOURCE}}` -- the named contended resource this agent touches.
- `{{RESTORE}}` -- concrete restore-to-health steps for that resource.
- `{{REPORT_PATH}}` -- usually `reports/<date>-{{LABEL}}.md`.

---

## Coordination rules (you are {{LABEL}})

You are one of several agents working this tree in parallel.

**Ownership.** You own, and may edit freely: {{OWN}}.
You may append to, but never rewrite: {{SHARED_ADDITIVE}}.
Everything else belongs to another agent. If a change is needed in a file
you do not own, describe it -- do not make it -- in your report under
"Changes needed in files you do not own". If an Edit fails because the
file changed underneath you, re-read the file and re-apply your change on
top of what is there. Never overwrite a peer's edit.

**Shared resources.** Every command that touches {{RESOURCE}} runs under:

    with-lock --resource {{RESOURCE}} --label {{LABEL}} -- <command>

Check contention with `--status`. On exit 75, wait and retry -- never
bypass the lock, never run the command bare. Hold the lock for the
shortest window that keeps your work coherent; chunk long campaigns.

**A lock protects a critical section, not a command.** If two commands
must not interleave, they go inside one acquisition -- otherwise a peer
acts in the gap between them. The classic case is version control: a
locked `git add` followed by a separate locked `git commit` lets a peer
stage into the same index in between, so the bare commit sweeps in their
files. Use `agent-commit`, which does the whole thing in one acquisition:

    agent-commit --resource {{RESOURCE}} --label {{LABEL}} \
      -m "<imperative subject>" -- <your paths>

It handles the wrinkles you would otherwise have to remember: one lock
acquisition for staging and committing together, and `--intent-to-add`
for a file git has not seen yet (a pathspec commit cannot name an
untracked path). It cannot protect you from committing a peer's
uncommitted edits to a file you *both* touched -- nothing can, short of
interactive staging -- so do not touch a file you do not own.
Verify your logic offline before taking the lock, so you do not burn a
lock window on a crash. After destructive work, restore the resource to
health before your wrapped command exits: {{RESTORE}}.

**Reporting.** Write {{REPORT_PATH}} before you finish. Mandatory
sections: what you did and how you verified it; "Changes needed in files
you do not own"; artifacts you produced and where a peer finds them.
