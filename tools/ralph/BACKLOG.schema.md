# Ralph backlog schema v2

`.ralph/BACKLOG.md` is the ordered source of truth. Start it with:

```markdown
<!-- ralph-backlog: v2 -->
# Backlog

- [ ] **12 — Ship weighted selection.** @opus — cross-cutting.
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

## Model tier

Any task — a leaf or a staged parent — may name the model it wants in a fixed
slot immediately after the closing `**`, closed by an em dash before the prose:

```markdown
- [ ] **12 — Rework the shared base.** @opus — big, cross-cutting change.
  Verify: cargo test
```

Tier decorations (`@haiku`, `@sonnet`, `@opus`) and known model families or
aliases (`@astra`, `@fable`, `@claude-fable-5-1`) are accepted. Use `!astra` or
`!fable` (also spelled `@!astra` or `@!fable`) to require that model exclusively:
no provider failover, overload fallback, or automatic model escalation. An
exclusive task annotation outranks one-shot overrides. Fable resolves to
`claude-fable-5-1`, Astra to `gpt-6-astra`; explicit versioned IDs stay unchanged.
Each task may have at most one decoration, and only in that slot — an `@opus` anywhere else is prose. A misspelling, a delimiter
that is not ` — `, or a leftover v1 parenthetical such as `(opus/pedagogy.)` in
the body is a lint error rather than a silently dropped route.

The slot is position-anchored, so task prose may not begin with `@` or `!`: in
`**1 — Notify the team.** @channel — ping everyone.` the `@channel` is read as a
decoration and rejected. Rephrase so `@` is not the first token — `**1 — Notify
the team.** Ping @channel before the release.`

## Selection and staging

Ralph selects the first unchecked task with no unchecked descendants, in
document order. A parent with pending children is a container; after its children
finish, the parent becomes the integration/closure step.

PROGRESS is a runner-owned carry-forward note, injected verbatim with no `Next:`
parsing and no id matching: it may clarify the selected task, never reroute. If
a leaf is too large for one iteration, add ordered child stages with their own
`Verify:` contracts — `ralph add --under <id> "<title>" --verify "<cmd>"` — and
run `ralph lint`. Do not keep routing slices only in PROGRESS.

## Never edit this file by hand

The running loop owns `BACKLOG.md`. Mutate it only through the CLI, which
schema-checks the result and rejects anything that would not lint:

```bash
ralph add [<id>] "<title>" --verify "<cmd>"   # <id> places a child, e.g. 3.1.1
ralph add --under <parent> "<title>"          # auto-numbers the next <parent>.N
ralph done <id>                               # check off
ralph uncheck <id>                            # reopen
ralph drop <id> [--recursive]                 # remove (archived, not deleted)
```

While a loop is running these **queue** to `.ralph/inbox/` and apply at the next
iteration boundary, so the backlog can never shift under a running agent. A hand
edit has no such protection and is silently overwritten.

## Validate before running

```bash
ralph lint                 # diagnostics and selected executable leaf
ralph brief                # lint plus the bounded context sent next
ralph                      # validates again before every iteration
```

`ralph lint` exits 0 when there are no errors (warnings are allowed) and 1 for
schema errors. A marked backlog requires valid `Verify:` contracts. Ralph
will not launch an iteration while schema errors remain.
