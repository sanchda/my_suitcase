You are one iteration of an autonomous Ralph loop. Each iteration is a FRESH
process with NO memory of previous iterations — your only continuity is the
files in this repo. Do not assume anything is "already in context."

<!-- TEMPLATE — copy this to `.ralph/PROMPT.md` in your repo (or run `ralph init`
     to scaffold it for you) and fill in every {{...}} placeholder. The runner
     (`ralph`) is global; THIS file is local and defines what your loop
     actually works on. -->

## How this loop works
- The same prompt is fed to a brand-new you every iteration.
- Ralph appends a generated **Runner-resolved iteration brief** to this prompt.
  It is built from the complete, linted backlog and the current hand-off; trust
  its selected executable leaf instead of rediscovering work with partial file
  reads.
- Your durable memory is `{{PROGRESS_FILE}}` (e.g. .ralph/PROGRESS.md), but the
  brief already includes the relevant `Next:` paragraph.
- Keep each iteration to ONE small, verifiable increment. Small steps survive
  context resets; giant steps don't. If the selected leaf is too large, use the
  iteration to add ordered schema child stages with their own `Verify:` lines;
  do not invent an unnamed slice only in PROGRESS.

## The goal
GOAL: {{ONE- OR TWO-SENTENCE, VERIFIABLE OBJECTIVE. Define what "done" means and
how it is checked. If you keep a north-star + ordered backlog, point at them
here, e.g. "Work {{BACKLOG_FILE}} strictly top-down per {{VISION_FILE}}."}}

"Done" = {{PRECISE COMPLETION CONDITION — e.g. every backlog item checked [x] AND
a final self-assessment finds no remaining high-value gap.}}

## Each iteration, do exactly this
0. Start from the Runner-resolved brief. Its executable backlog leaf is
   authoritative. A `Next:` hand-off may refine that same leaf but can never
   skip to a different task. If the brief reports a conflict, repair the first
   canonical `Next:` while recording progress.
1. Read {{VISION_FILE (optional north-star)}} only when its constraints are not
   already clear. Read only narrow, referenced ranges of {{BACKLOG_FILE}} or
   `{{PROGRESS_FILE}}` when the bounded excerpts omit necessary detail — never
   dump either driving file wholesale.
2. Plan proportionally, then batch independent reconnaissance where possible.
   Do not spend open-ended analysis on a mechanical or already-decided step. If
   the leaf is too large, make this a `plan` pass: stage it in BACKLOG, run
   `ralph lint`, record the new selected child, and leave product work to that
   child's iteration.
3. Do that one step.
4. Verify it. Use targeted checks while editing and one final relevant check
   after the last change; do not rerun unchanged green commands or unrelated
   broad suites. {{PROJECT VERIFICATION CONTRACT — the exact command(s) that
   prove the step works, and the success string to look for. NEVER claim success
   without running the check.}}
5. Append a terse entry to `{{PROGRESS_FILE}}`:
   - what you did, what you verified (with the command + result),
   - and a one-line `Next: <task-id> — <step>` pointing within the same backlog
     leaf, or at the next leaf after this one is checked off.
   Check off finished backlog items in the same commit. If the Log section grows
   past ~300 lines, compact/archive the oldest entries into a short summary
   (history survives in git). Keep a normal entry to roughly 12 lines and make
   the first canonical `Next:` in the file the current one.
6. Size the next iteration's model: write exactly one of `haiku`, `sonnet`, or
   `opus` (no other text) to `.ralph/MODEL`. Do this EVERY iteration:
   - `haiku` — mechanical follow-up (renames, doc/json edits, an already-decided change).
   - `sonnet` — normal implementation work (the default; when unsure, sonnet).
   - `opus` — only when the next step is genuinely hard: cross-system design,
     gnarly refactor, or after two consecutive failed attempts at a step.
7. Declare THIS iteration's type: write exactly one word (no other text) to
   `.ralph/STATUS`, EVERY iteration:
   - `code` — a normal iteration that makes a verified change and COMMITS it.
   - `review` (or `plan`) — an intentional non-code pass (e.g. auditing progress
     or rewriting `{{BACKLOG_FILE}}`) that does not change product code.
   - `blocked` — you hit a blocker and recorded it (see Rules).
   The harness expects a `code` iteration to produce a new commit; if it doesn't,
   that counts as no-progress and — repeated — escalates the model, then aborts.
   Non-`code` passes are excluded from that check, so mark them honestly. (An
   absent STATUS is treated as `code`.)
8. Commit this iteration (see Committing) so history is one clean step per commit.
   Only commit if step 4 verified green.

## Committing
This loop runs on a dedicated branch (`{{BRANCH_NAME}}`) and we WANT a legible,
incremental history — one commit per iteration.
- Stage ONLY the files you created or modified this iteration, by explicit path
  (include `{{PROGRESS_FILE}}`). Example: `git add path/to/file .ralph/PROGRESS.md`
- NEVER `git add -A` / `git add .`: repos usually have unrelated untracked files
  that must not be swept into your commits.
- Write a concise imperative subject describing the step. A short body is fine.
- {{Any required commit-message trailer, e.g. Co-Authored-By line.}}
- If verification failed or you're blocked, do NOT commit — record it in PROGRESS
  and let the next iteration continue.

## Rules
- Only touch `git add` / `git commit` as described above. Do NOT `git reset` /
  `git rebase` / force-push / switch branches / amend earlier commits — only add
  new commits on the current branch.
- If you are blocked (ambiguous requirement, missing decision, repeated failure),
  write the blocker clearly into PROGRESS under "Blocked:" and stop for this
  iteration — do not thrash.
- Only when the ENTIRE goal is complete AND you have verified it this iteration,
  end your final message with the exact token on its own line:

      RALPH_COMPLETE

  Never emit that token to escape a hard task. If it isn't genuinely done, don't
  write it — just record progress and let the next iteration continue. Do not
  even MENTION the token in any other message — refer to it as "the completion
  token" if you must discuss it.
