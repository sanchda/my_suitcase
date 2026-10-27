---
name: ensemble-pairwise-ranking
description: Use when an ordering has to be defensible and one pass won't settle it — reordering a plan by expected value, ranking candidate designs, triaging findings by severity — and especially when the order must not inherit the priors of whoever wrote the source document, or when a single model's ranking of 6+ items has proven unstable. Not for short lists, easily reversed calls, or anything with an objective metric.
---

# Ensemble pairwise ranking

A panel of voters judges **every pair** of items against **one fixed criterion**; per-pair
majorities become a Copeland order. The `panel` binary runs the whole thing.

## Don't reach for this by default

The honest default is no. Rank it yourself and say what you weighted.

| Don't use it when | Instead |
|---|---|
| Fewer than ~6 items | Judge it in one pass and show your reasoning. The panel cannot beat you by enough to matter. |
| The order is cheap to revise later | Pick one, ship, reorder when you learn something. |
| There is an objective metric | Measure it. A vote on a measurable quantity is strictly worse than measuring. |
| Someone legitimately owns the call | A tech lead's priorities are not an estimation error to be averaged out. |
| You need an answer in a minute | Minutes and real tokens. Run `--pilot` first if unsure. |
| The items aren't comparable on one criterion | Read **Estimator or parliament** before anything else. |

**It costs real money.** `panel` prints the exact figure, and `--pilot` buys that number before
you commit to the full run:

| items | pairs | votes at N=5 | model calls (batch 6) |
|---|---|---|---|
| 6 | 15 | 75 | 20 |
| 8 | 28 | 140 | 30 |
| 12 | 66 | 330 | 60 |
| 16 | 120 | 600 | 105 |
| 20 | 190 | 950 | 165 |

Past ~16 items the quadratic bites. Bucket into tiers and rank within tiers.

## Why this works, and what breaks it

Two effects stack:

- **Pairwise beats pointwise.** "Which of these two?" is a strictly easier task than "rank these
  twelve." Judges are measurably better at it.
- **Condorcet.** N judges each better than chance on a binary call, **with independent errors**,
  aggregate above any one of them. At 0.7 individual accuracy, a 5-panel majority lands near 0.84.

Condorcet governs each binary call. Copeland over those calls is a *heuristic* aggregation — it
inherits the per-pair lift but carries no guarantee about the ranking as a whole. That gap is why
cycles get reported rather than smoothed away.

The second effect is entirely conditional on **independent errors**. Five identical prompts to five
instances of one model are near-perfectly correlated: you pay 5× and collect almost none of the
lift. **The design work is decorrelating the voters, not running the vote.**

Two levers, in this order.

### Expertise comes from pre-context, not from model strength

Give each voter a different slice of **real material** to read — `context` in the manifest. This
is the deliberate source of variance, and it is the one that actually works:

| Panel (3 voters, identical model, identical prompt) | Unanimity | Concordance |
|---|---|---|
| Same evidence for all three | 1.00 | 1.00 / 1.00 / 1.00 — three copies |
| A different slice each | **0.50** | **0.75 / 0.60 / 1.00** |

Only the evidence changed. Persona *wording* barely moves this; different material moves it a lot.

Non-overlapping slices decorrelate best — the moment two voters read the same file, they share
its blind spots. Prefer conclusions and tables over raw dumps: the context is re-sent with every
ballot, though as a stable prompt prefix it is cached after the first, so size costs less than it
looks. `panel` refuses to proceed quietly if a voter's context reproduces most of the item texts
verbatim — that means the source document has arrived through the back door, ordering and all.

`bootstrap` (a research task run once per voter) remains available for when you have no material
to hand, but it is nondeterministic, costs a model call, and can wander into the source. Prefer
`context`.

### Fix the level, vary the provider

Providers are free decorrelation: a frontier Anthropic model and a frontier OpenAI model bring
genuinely different weight matrices. claude/codex, codex/codex and claude/claude panels are all
fine. What must not vary is the **level**.

Not a style preference — it is what makes the majority rule valid at all. Condorcet assumes
voters of *comparable* accuracy, because simple majority is only the right aggregator when
accuracies are roughly equal. (The optimum otherwise is log-odds weighting, and you do not know
the odds in advance.) Mix tiers and the weak bloc does not add noise, it **outvotes** the strong
voters:

| Panel (independent accuracies) | Majority | Best voter alone |
|---|---|---|
| 5 peers @0.90 | **0.99** | 0.90 |
| 3 peers @0.90 | 0.97 | 0.90 |
| 2 @0.90 + one coin flip | 0.90 | 0.90 — you paid for nothing |
| 1 @0.90 + 2 @0.60 | **0.79** | 0.90 |
| 1 @0.90 + 1 @0.60 + 3 @0.52 | **0.71** | 0.90 |

One expert, a monkey and three invertebrates is not a panel of five. It is an expert being
outvoted, and it lands 19 points below just asking the expert. `panel` warns on any manifest
mixing known levels — including `opus` with `sonnet`, not just the obvious cases — before
spending anything.

A *uniformly* cheap panel is a different matter and is not flagged: those voters are peers, so the
lift is real, just anchored to a lower individual accuracy. The fix for a weak panel is better
evidence, not a stronger voter bolted onto the side.

## The protocol

```bash
# 1. Write the criterion FIRST, before anything reads the source.
$EDITOR criterion.txt

# 2. The analyst — the only role that reads the source — emits the manifest.
#    Items verbatim; strip the source's own ordering, phase labels and rationale.

# 3. Check the plan and the bill before committing.
panel schedule manifest.json
panel run manifest.json --criterion criterion.txt --pilot 6

# 4. The real run. Keep the manifest and output out of the repo being ranked,
#    and name the source so panel can confirm voters cannot reach it.
panel run manifest.json --criterion criterion.txt --out panel-run \
      --source ../plans/q3-roadmap.md       # refuses if that is inside this repo

# 5. Only now open the manifest and write up the result.
```

You see the item text only after the votes are sealed. `panel` reports a ranking of opaque ids
precisely so that reading it does not require reading the items — if you open the manifest early,
you become the global ranker the panel was built to replace, and the whole run is theater that
confirms you.

### The manifest

```json
{
  "preamble": "<definitions shared by every item, sent once per ballot>",
  "items": [
    {"id": "a1", "text": "<verbatim, unparaphrased>"}
  ],
  "dropped": [],
  "personae": [
    {"id": "v1", "lens": "measurement precision", "context": ["experiments/precision.md"], "model": "opus"},
    {"id": "v2", "lens": "repeatability",         "context": ["experiments/repeats.md"],     "model": "gpt-5"},
    {"id": "v3", "lens": "resource use",          "context": ["benchmarks/resources.md"],    "model": "opus"}
  ]
}
```

- **The analyst never chooses which items get ranked.** It must emit *every* candidate it saw,
  and list anything it excluded in `dropped` with a reason. `"dropped": []` asserts it excluded
  nothing; a manifest with no `dropped` key at all is refused.

  This is the gap the quadratic opens and the arithmetic hides. Told the budget is 8–14 items,
  an analyst handed 30 candidates will keep the "important" ones and say so in passing — a
  priority judgment, made by the one component that read the source, invisible to everything
  downstream, and indistinguishable in the output from a clean run. **It is the contamination
  the whole separation of duties exists to prevent, arriving through the one door nobody
  watched.** Rank all of them, or tier and rank within tiers. If you genuinely must subset,
  declare the rule yourself with `--subset`, and note that a rule you can only apply by reading
  the item texts is you doing the ranking.
- **Hoist shared definitions into `preamble`.** Text common to both sides of a pair cannot
  inform the comparison — it only pushes the two things being compared further apart. Nine
  items opening with the same 468-character definition means most ballots carry ~470 identical
  characters on each side. `panel` warns when half the items share a long prefix.
- **Items verbatim.** Paraphrase is reframing, and reframing is your priors entering through the
  back door.
- **Derive personae from the system under evaluation, not from the source document.** A panel whose
  lenses span the document's own dimensions inherits its blind spots, then launders them back out
  as independent consensus.
- **Give every voter its own `context`.** This is where the variance comes from. One shared file
  across the panel buys you nothing.
- **Spread `model` across providers at one level.** Free decorrelation; mixing *levels* instead
  breaks the majority rule (see **Fix the level**).
- The criterion is deliberately *not* in this file — see below.

## What the tool enforces, and what is still on you

`panel` refuses rather than warns, so these stop being matters of willpower:

| Invariant | How it holds |
|---|---|
| Criterion is source-independent | It comes from `--criterion`, and a manifest containing a `criterion` field is **rejected** — that would mean the analyst wrote it from the document. |
| Criterion is byte-identical everywhere | One string, read once, copied into every ballot. |
| Panel size is odd | Even panels are rejected at load. |
| Position bias is counterbalanced | Orientation alternates on `(pair + voter) % 2` — every pair splits 3/2 across a 5-panel. |
| Votes never cross | Every call is a separate process with a fresh context. There is no channel between voters, and no artifact is written to `--out` until the last ballot returns — so there is nothing to read across the panel mid-run either. |
| Voters read the repo, not the plan | Voters start in the working tree by design. Panel refuses to run while the manifest, any `--source`, or a previous run's ballots are inside it. Fatal, because the failure it prevents is silent. |
| The analyst can't subset silently | `dropped` is a required field. Dropping candidates without `--subset` is fatal, and the declared rule is recorded with the run. |
| Boilerplate is caught | Half the items sharing a long prefix warns — that text is on both sides of every pair it touches. |
| Deleting the plan isn't enough | A `--source` that appears anywhere in the repo's git history is flagged — `git log -p` outlives a `mv`. |
| No drift across rounds | Fresh context per ballot, so there is no accumulated history to drift. |
| Aggregation has no discretion | Copeland is computed, not read off by something with an opinion. |
| Runs reproduce | The schedule is a pure function of `--seed`. |
| Mixed-level panels are caught | Any manifest spanning two known levels warns before the first call. Name-based, so it is fallible — an unknown model is left unclassified rather than guessed at. |
| The source can't arrive as pre-context | A voter whose `context` reproduces most of the item texts verbatim is flagged. Content-based, so renaming or excerpting the file does not evade it. |

### Where the voters run

**In the repository.** That is the point of a voter over a prompt: it can read the code, see
which module has no tests, and find the TODO from 2023 admitting this path has taken checkout
down three times. Those are the constraints of reality, and a judgment grounded in them beats
one reasoning from the item text alone.

What must not be in reach is **the plan**. A voter that finds it returns fluent, confident
ballots that merely re-derive the ordering it already had, and nothing downstream can detect
this — the run looks exactly like a successful one.

So the repository stays and the plan leaves. `--source` is how you make that a fact rather
than an intention: panel refuses to start while any declared source, or the manifest, is
inside the tree voters read.

```
panel: voters read this tree (/path/to/repo), and these are inside it:
  --source     /path/to/repo/docs/ROADMAP.md
```

Three residual holes, all handled:

- **No `--source` given** → warns. Panel cannot check a location it was never told.
- **The plan was moved out but had been committed** → warns. `git log -p` still has it.
- **A previous run's `votes.jsonl` or `briefs.json` sitting in the repo** → refuses. Those are
  other voters' ballots, readable right now.

`--isolate` runs voters in an empty directory instead. It buys the same guarantee by giving up
everything the voters would have learned, so reach for it only when the plan genuinely cannot
be moved out.

**Still entirely on you:**

1. Not opening the manifest until the votes are sealed.
2. The criterion wording.
3. The persona set, its size, and the model spread.
4. Estimator vs parliament.
5. What the analyst strips.

## Estimator or parliament — decide this explicitly

| | **Estimator** (default) | **Parliament** |
|---|---|---|
| Voters differ in | evidence base; criterion identical | the objective itself |
| Theory | Condorcet — converges on one latent truth | Arrow — there is no common truth |
| Cycles | noise; rare; worth reporting | structural; guaranteed |
| Use when | one latent quality is being measured | the quality is irreducibly multi-criteria |

Different objectives per voter produce strong, legible individuality and destroy the Condorcet lift
at the same time — there is nothing left to converge on. Default to the estimator: identical
criterion, different empirical pictures. Choose parliament deliberately, knowing cycles stop being
noise and the ranking stops being an estimate of anything.

### The parliament you did not choose

That table makes the choice look like something you make by varying the criterion. **You can
land in a parliament with the criterion byte-identical, purely through how you assign evidence.**

If the lenses partition along the same axes as the items — one voter owns the gateway and there
is a gateway item, another owns the codec and there is a codec item — then each voter can only
make confident claims about its own slice. The criterion never moved. The question each voter
was *able to answer* did. The output is parliament-shaped: cycles, and voters reliably preferring
whichever item sits nearest their own evidence.

The tell is in the reasons — *"the evidence is silent on X"*. `panel` counts those, reports a
silence rate per voter, and names this failure when high silence coincides with cycles:

```
DEGENERATE PARLIAMENT: cycles, and voters report their evidence silent on
33% of pairs. ... the lenses line up with the item set ... the cycles are an
artifact of the panel, not a property of the items.
```

**Lenses must cut across the item set, not align with it.** Each voter's evidence should span
several subsystems rather than owning one. A lens that maps one-to-one onto an item is not a
lens, it is an advocate — and a panel of advocates has no latent truth to converge on.

## Pilot before you buy the whole run

`panel run --pilot 6` runs six pairs and stops. Read the **unanimity rate**:

- **Near 100%** → individuation failed. You are paying N× for one opinion. Spread the models
  further or rewrite the lenses. `panel` warns about this above 0.9.
- **Healthy split with a stable majority** → proceed.
- **Near-random everywhere** → the criterion is ambiguous, not the panel. Rewrite `criterion.txt`
  before spending anything.

Then read **voter concordance** — each voter's agreement with the majority of the *others*,
computed leave-one-out so a voter cannot inflate its own score:

- **Near 1.0** → redundant. That voter is not contributing independent evidence; you are paying
  for a copy.
- **Near 0.5** → either a weak voter or a genuinely orthogonal one, and the number alone cannot
  tell you which. Read its reasons. If they are thin, it is the monkey.

This is the empirical version of the peer check, and the only one that survives new model names.

## Reading the output

Copeland score is the headline order. Two things not to discard:

- **Margins.** A 5–0 win is stronger evidence than 3–2, and the Copeland score alone throws that
  away. They are in `votes.jsonl` and the JSON report.
- **Cycles.** `panel` prints each intransitive component with the pairwise results and the voters'
  reasons. These are **findings** — the clearest signal you have that the criterion is doing
  something different in different regions of the item set. Report them; do not smooth them over.

## GOTCHAS

| Trap | Reality |
|---|---|
| Reading the source "just to set things up" | The design is void. You are the global ranker now, and the panel only confirms you. |
| Running the panel on 4 items | You bought a quadratic apparatus to do what one careful pass does. Rank it yourself. |
| Five voters, one model, five personae | The thing you are paying for is error independence, and persona wording barely buys any. Give them different `context`. |
| One shared context file for the whole panel | Measured: unanimity 1.00, every voter flagged redundant. You convened a panel and asked it one question five times. |
| A stronger model added to "raise the average" | Majority rule has no average. The weaker bloc still outvotes it. Fix the level; raise quality with evidence. |
| Padding the panel with a cheap model to reach odd N | A weak bloc outvotes the strong voters; the panel lands below its own best member. Drop to a smaller panel of peers instead — 3 peers @0.90 beats 5 mixed every time. |
| Unanimity read as success | ~100% unanimity means the personae aren't doing work. It is the failure signal, not the win condition. |
| Personae derived from the source document | Panel inherits the document's blind spots, then certifies them as consensus. |
| Paraphrasing items "for clarity" | Reframing. Verbatim or nothing. |
| Criterion written after reading the source | Not source-independent. `panel` rejects a manifest that carries one, but it cannot see what you read before writing `criterion.txt`. |
| Bootstrap briefs full of file dumps | Dilutes the pair under judgment, and the brief is re-sent with every ballot — it is the one input that multiplies. Capped at 250 words. |
| `--batch` cranked up to cut cost | Past ~8 a voter starts ranking the batch globally instead of judging pairs. The ballot prompt pushes back; don't lean on it. |
| Keeping the plan in the repo you run `panel` from | Voters read that repo on purpose. The plan is then one `grep` away, and a voter that found it produces fluent ballots re-deriving the order it already had. Pass `--source` so panel can refuse. |
| Letting the analyst trim to fit the budget | The one component that read the source choosing what gets ranked. Looks like housekeeping, is priority-setting, and leaves no trace in the output. |
| Lenses named after the items | One voter per subsystem is a panel of advocates. You get cycles, and they tell you about your lenses rather than your items. |
| Reaching for `--isolate` to be safe | It works by making the voters ignorant. You keep the vote and throw away the grounding that made it worth holding. Move the plan instead. |
| Reordering the result to satisfy dependencies | Overrides the votes with your topology. Let the vote order stand; annotate violations beneath it. |
| Treating the ranking as more than an estimate | It is a measurement with a known error model, not a verdict. Publish the criterion and the panel alongside it. |
