# Ralph backlog schema v1

`.ralph/BACKLOG.md` is the ordered source of truth. Start it with:

```markdown
<!-- ralph-backlog: v1 -->
# Backlog

- [ ] **12 — Ship weighted selection.**
  Describe the outcome and constraints.
  Verify: cargo test
  - [x] **12.1 — Emit weights.**
    Verify: cargo test generator
  - [ ] **12.2 — Consume weights.**
    Verify: ./tools/verify_runtime.sh weights
```

## Task rules

- Use `- [ ] **ID — Title.**` for pending work and `[x]` for complete work.
- IDs are unique and contain only letters, digits, `.`, `_`, or `-`; use an em
  dash between ID and title.
- Every pending task—including a staged parent—needs a non-placeholder
  `Verify:` command or success check. The parent's contract is its final closure
  gate.
- Put concise shared constraints and the parent `Verify:` before its children;
  free prose under a heading is not injected into child briefs. Indent each child
  exactly two spaces and prefix its ID with `<parent-id>.`.
- Checkboxes route work. Put task-looking examples inside fenced code blocks.

## Selection and staging

Ralph selects the first unchecked task with no unchecked descendants, in
document order. A parent with pending children is a container; after its children
finish, the parent becomes the integration/closure step.

The first `Next: <id> — <step>` in PROGRESS may refine only the selected ID; it
cannot reorder the backlog. If a leaf is too large for one iteration, add ordered
child stages with their own `Verify:` contracts and run `ralph lint`. Do not keep
routing slices only in PROGRESS.

## Validate before running

```bash
ralph lint                 # diagnostics and selected executable leaf
ralph brief                # lint plus the bounded context sent next
ralph                      # validates again before every iteration
```

`ralph lint` exits 0 when there are no errors (warnings are allowed) and 1 for
schema errors. A marked v1 backlog requires valid `Verify:` contracts. Ralph
will not launch an iteration while schema errors remain.
