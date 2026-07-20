# Ralph backlog schema v1

The schema is intentionally ordinary Markdown. It gives the runner one stable,
lintable definition of task order while leaving descriptions free-form.

Start the file with the version marker:

```markdown
<!-- ralph-backlog: v1 -->
```

Every task is a checkbox with a unique machine-friendly ID and a bold label:

```markdown
- [ ] **36.8 — Introduce schema v2.**
  Explain the outcome and important constraints here.
  Verify: cargo test -p generator
```

The required pieces are:

- pending (`[ ]`) or complete (`[x]`);
- a unique ID made from letters, digits, `.`, `_`, or `-`;
- an em dash (`—`) between the ID and title;
- a `Verify:` contract in every pending task (including a staged parent, whose
  contract is its eventual integration/closure gate). Empty values, TODO/TBD,
  and `{{placeholder}}` values fail lint.

Text, headings, links, fenced code blocks, and non-checkbox lists remain
free-form. Checkboxes—including task-looking indented examples—are reserved for
schema tasks; put checkbox examples inside a fenced code block.

## Ordered stages

Large tasks can be staged explicitly with two-space-indented child tasks. A
child ID starts with its parent's ID plus a dot:

```markdown
- [ ] **36.8 — Ship weighted selection end-to-end.**
  This parent becomes the final integration/closure step after its children.
  Verify: cargo test && ./tools/verify_runtime.sh
  - [x] **36.8.1 — Define and emit the schema.**
    Verify: cargo test -p generator schema
  - [ ] **36.8.2 — Consume weights at runtime.**
    Verify: ./tools/verify_runtime.sh weighted_selection
  - [ ] **36.8.3 — Add cross-runtime distribution gates.**
    Verify: cargo test -p generator distribution && ./tools/verify_runtime.sh distribution
```

Resolution is deterministic and depth-first:

1. Ralph walks tasks in document order.
2. A pending parent with pending descendants is a container, not executable.
3. The first pending task with no pending descendants is selected.
4. Once every child is complete, the still-pending parent is selected for its
   integration verification and closure.
5. `PROGRESS.md`'s first `Next:` may refine the selected task only when it names
   that exact ID; it can never skip to another task, and later historical
   hand-offs are not searched as fallbacks.

If an executable leaf proves too large for one iteration, the iteration should
be a planning pass that adds its ordered child stages and runs `ralph lint`.
Do not maintain a parallel sequence of unnamed “slice 1/2/3” hand-offs only in
PROGRESS; stages that affect routing belong in the backlog.

Keep parent prose and its `Verify:` line before its first child; prose after a
child belongs to that child. Any nesting depth is allowed, always at two spaces
per level.

## Commands

```bash
ralph lint                 # schema errors, routing conflicts, selected leaf
ralph brief                # exact bounded context the next iteration receives
```

The loop refuses to launch a new Claude iteration when the backlog has schema
errors. A missing version marker is only a compatibility warning, so existing
well-formed checkbox backlogs keep running during migration. Compatibility mode
aggregates missing `Verify:` fields into a warning; add real contracts before
adding the marker, because marked v1 files enforce them. The linter also warns
when a checked task appears after a pending sibling: that does not block
execution, but it exposes that document order was bypassed.
