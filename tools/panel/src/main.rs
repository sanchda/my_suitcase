//! panel — ensemble pairwise ranking with decorrelated one-shot voters.
//!
//! A panel of voters judges every pair of items against one fixed criterion;
//! per-pair majorities become a Copeland ranking. The point is not the vote, it
//! is the decorrelation: N judges aggregate above any one of them only when
//! their errors are independent, so this driver spreads voters across models and
//! providers, gives each a different evidence base, counterbalances position and
//! shuffles order per voter.
//!
//! The orchestrating agent stays outside the loop entirely. It writes the
//! criterion, runs the analyst, runs `panel`, and reads a ranking of opaque ids
//! — it never sees the item text until the votes are sealed, which is what keeps
//! it from quietly becoming the single global ranker the panel exists to replace.

mod isolation;
mod manifest;
mod pairing;
mod prompt;
mod runner;
mod tally;
mod vote;

use manifest::Manifest;
use runner::Usage;
use std::path::{Path, PathBuf};
use vote::{Side, Vote};

pub type R<T> = Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "\
panel — ensemble pairwise ranking with decorrelated one-shot voters

Usage:
  panel run <manifest.json> --criterion <file> [options]
  panel tally <votes.jsonl> [--manifest <file>] [--json]
  panel schedule <manifest.json> [--seed <n>]      Print the plan, call nothing

Run options:
  --criterion <file>   The ranking criterion. REQUIRED, and written before the
                       analyst reads the source: a criterion derived from the
                       document being ranked is not source-independent.
  --out <dir>          Run directory (default: panel-run)
  --model <name>       Default voter model; per-voter `model` in the manifest wins
  --batch <n>          Pairs per ballot (default 6). Higher is cheaper; too high
                       and a voter starts building a global ranking.
  --concurrency <n>    Parallel model calls (default 8)
  --seed <n>           Shuffle seed (default 1). Same seed, same schedule.
  --timeout <secs>     Per-call wall clock (default 180)
  --pilot <n>          Run only the first n pairs, report unanimity, then stop
  --source <path>      A file the analyst read. Repeatable, and otherwise unused:
                       it exists so panel can refuse to run while voters could
                       read it. Strongly recommended — without it the plan's
                       location cannot be checked.
  --voter-cwd <dir>    Where voters run (default: here). They are meant to read
                       the repo; the manifest and every --source must sit outside
                       it so that reading the repo cannot reach the plan.
  --isolate            Run voters in an empty directory instead. Use when the
                       plan cannot be moved out of the repo — it buys safety by
                       giving up everything the voters would have learned.
  --subset <rule>      Required when the manifest drops candidates. States the
                       rule YOU applied, recorded with the run. Subsetting is
                       the orchestrator's call under a declared rule, never a
                       judgment the analyst makes while reading the source.
  --no-bootstrap       Skip the research phase; use briefs already in the manifest
  --json               Machine-readable report on stdout
";

fn main() {
    if let Err(e) = dispatch() {
        eprintln!("panel: {e}");
        std::process::exit(1);
    }
}

#[derive(Default)]
struct Opts {
    criterion: Option<String>,
    manifest: Option<String>,
    out: PathBuf,
    model: String,
    batch: usize,
    concurrency: usize,
    seed: u64,
    timeout: u64,
    pilot: Option<usize>,
    no_bootstrap: bool,
    as_json: bool,
    source: Vec<String>,
    voter_cwd: Option<String>,
    isolate: bool,
    subset: Option<String>,
}

fn dispatch() -> R<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        print!("{USAGE}");
        return Ok(());
    }
    let command = args[0].clone();
    let positional = args.get(1).cloned().unwrap_or_default();

    let mut o = Opts {
        out: PathBuf::from("panel-run"),
        model: "sonnet".into(),
        batch: 6,
        concurrency: 8,
        seed: 1,
        timeout: 180,
        ..Opts::default()
    };
    let mut i = 2;
    while i < args.len() {
        let flag = args[i].as_str();
        let mut value = || -> R<String> {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value").into())
        };
        match flag {
            "--criterion" => o.criterion = Some(value()?),
            "--manifest" => o.manifest = Some(value()?),
            "--out" => o.out = PathBuf::from(value()?),
            "--model" => o.model = value()?,
            "--batch" => o.batch = value()?.parse().map_err(|_| "bad --batch")?,
            "--concurrency" => o.concurrency = value()?.parse().map_err(|_| "bad --concurrency")?,
            "--seed" => o.seed = value()?.parse().map_err(|_| "bad --seed")?,
            "--timeout" => o.timeout = value()?.parse().map_err(|_| "bad --timeout")?,
            "--pilot" => o.pilot = Some(value()?.parse().map_err(|_| "bad --pilot")?),
            "--source" => o.source.push(value()?),
            "--voter-cwd" => o.voter_cwd = Some(value()?),
            "--isolate" => o.isolate = true,
            "--subset" => o.subset = Some(value()?),
            "--no-bootstrap" => o.no_bootstrap = true,
            "--json" => o.as_json = true,
            other => return Err(format!("unknown flag {other}").into()),
        }
        i += 1;
    }
    if o.batch == 0 {
        return Err("--batch must be at least 1".into());
    }

    match command.as_str() {
        "run" => run(&positional, &o),
        "tally" => tally_only(&positional, &o),
        "schedule" => schedule(&positional, &o),
        other => Err(format!("unknown command {other:?}\n\n{USAGE}").into()),
    }
}

fn schedule(path: &str, o: &Opts) -> R<()> {
    let m = Manifest::load(path)?;
    let pairs = pairing::all_pairs(m.items.len());
    println!(
        "{} items · {} pairs · {} voters · {} votes",
        m.items.len(),
        pairs.len(),
        m.personae.len(),
        m.votes_expected()
    );
    if let Some(w) = m.tier_warning(&o.model) {
        println!("\nWARNING: {w}\n");
    }
    let ballots = m.personae.len() * pairs.len().div_ceil(o.batch);
    println!(
        "batch {} → {} model calls (+{} bootstrap)\n",
        o.batch,
        ballots,
        if o.no_bootstrap { 0 } else { m.personae.len() }
    );
    for (v, persona) in m.personae.iter().enumerate() {
        let order = pairing::schedule_for_voter(pairs.len(), o.seed, v);
        let preview: Vec<String> = order
            .iter()
            .take(6)
            .map(|&p| {
                let (x, y) = pairs[p];
                let (x, y) = if pairing::a_first(p, v) { (x, y) } else { (y, x) };
                format!("{}>{}", m.items[x].id, m.items[y].id)
            })
            .collect();
        println!(
            "  {:<6} {:<28} {} …",
            persona.id,
            persona.model.as_deref().unwrap_or(&o.model),
            preview.join(" ")
        );
    }
    Ok(())
}

fn run(path: &str, o: &Opts) -> R<()> {
    let criterion_path = o
        .criterion
        .as_deref()
        .ok_or("--criterion <file> is required")?;
    let criterion = std::fs::read_to_string(criterion_path)
        .map_err(|e| format!("{criterion_path}: {e}"))?;
    if criterion.trim().is_empty() {
        return Err(format!("{criterion_path} is empty").into());
    }
    let mut m = Manifest::load(path)?;
    // Before a single token is spent: a mixed-tier panel is not a panel.
    if let Some(w) = m.tier_warning(&o.model) {
        eprintln!("panel: WARNING — {w}\n");
    }
    std::fs::create_dir_all(&o.out)?;

    if !m.dropped.is_empty() && o.subset.is_none() {
        let ids: Vec<&str> = m.dropped.iter().map(|d| d.id.as_str()).collect();
        return Err(format!(
            "the analyst dropped {} of {} candidates: {}\n\n\
             Which candidates get ranked is a priority judgment, and the analyst \
             is the one component that read the source — letting it choose is the \
             contamination this design exists to prevent.\n\n\
             Rank all of them, or tier them and rank within tiers. If you must \
             subset, declare the rule yourself with --subset \"<rule>\"; it is \
             recorded with the run. Note that a rule you can only apply by \
             reading the item texts is you doing the ranking.",
            m.dropped.len(),
            m.dropped.len() + m.items.len(),
            ids.join(", ")
        )
        .into());
    }
    if let Some(rule) = &o.subset {
        eprintln!(
            "panel: subsetting {} of {} candidates under a declared rule: {rule}",
            m.dropped.len(),
            m.dropped.len() + m.items.len()
        );
    }
    if let Some(boilerplate) = m.shared_prefix_warning() {
        eprintln!("panel: WARNING — {boilerplate}");
    }

    let contexts = load_contexts(&m)?;
    let voter_cwd = if o.isolate {
        eprintln!(
            "panel: note — voters isolated. They cannot read the repository, so \
             nothing they judge from is grounded in it."
        );
        isolation::isolated_cwd()?
    } else {
        let dir = PathBuf::from(o.voter_cwd.as_deref().unwrap_or("."));
        if !dir.is_dir() {
            return Err(format!("--voter-cwd {}: not a directory", dir.display()).into());
        }
        precheck_unreachable(&dir, path, o)?;
        dir
    };

    let mut total = Usage::default();
    let mut notes: Vec<String> = Vec::new();

    // Bootstrap: one research call per voter, conclusions capped. The brief is
    // re-sent with every ballot, so it is the one input that multiplies.
    let wants_bootstrap = !o.no_bootstrap
        && m.personae
            .iter()
            .any(|p| p.brief.trim().is_empty() && !p.bootstrap.trim().is_empty());
    if wants_bootstrap {
        eprintln!("bootstrap: {} voters", m.personae.len());
        let jobs: Vec<(usize, String, String)> = m
            .personae
            .iter()
            .enumerate()
            .filter(|(_, p)| p.brief.trim().is_empty() && !p.bootstrap.trim().is_empty())
            .map(|(n, p)| {
                (
                    n,
                    p.model.clone().unwrap_or_else(|| o.model.clone()),
                    prompt::build_bootstrap_prompt(p),
                )
            })
            .collect();
        let results = runner::pool(jobs, o.concurrency, |_, (n, model, text)| {
            (n, runner::run_oneshot(&model, o.timeout, &text, &voter_cwd))
        });
        for (n, result) in results {
            match result {
                Ok(c) => {
                    total.absorb(&c.usage);
                    m.personae[n].brief = prompt::cap_brief(&c.text);
                    eprintln!("  {} brief ok", m.personae[n].id);
                }
                Err(e) => {
                    notes.push(format!("{} bootstrap failed: {e}", m.personae[n].id));
                    eprintln!("  {} bootstrap failed: {e}", m.personae[n].id);
                }
            }
        }
    }

    // Ballots: every pair, every voter, batched and shuffled per voter.
    let all = pairing::all_pairs(m.items.len());
    let active: Vec<usize> = match o.pilot {
        Some(n) => (0..all.len().min(n)).collect(),
        None => (0..all.len()).collect(),
    };
    if o.pilot.is_some() {
        eprintln!(
            "pilot: {} of {} pairs — read the unanimity rate before committing",
            active.len(),
            all.len()
        );
    }

    struct Job {
        voter: usize,
        model: String,
        pair_ids: Vec<usize>,
        prompt: String,
    }
    let mut jobs: Vec<Job> = Vec::new();
    for (v, persona) in m.personae.iter().enumerate() {
        let order: Vec<usize> = pairing::schedule_for_voter(all.len(), o.seed, v)
            .into_iter()
            .filter(|p| active.contains(p))
            .collect();
        for chunk in order.chunks(o.batch) {
            let oriented: Vec<(&manifest::Item, &manifest::Item)> = chunk
                .iter()
                .map(|&p| {
                    let (x, y) = all[p];
                    if pairing::a_first(p, v) {
                        (&m.items[x], &m.items[y])
                    } else {
                        (&m.items[y], &m.items[x])
                    }
                })
                .collect();
            jobs.push(Job {
                voter: v,
                model: persona.model.clone().unwrap_or_else(|| o.model.clone()),
                pair_ids: chunk.to_vec(),
                prompt: prompt::build_ballot_prompt(
                    &criterion,
                    &m.preamble,
                    persona,
                    &contexts[v],
                    &oriented,
                ),
            });
        }
    }

    eprintln!(
        "ballots: {} calls across {} voters, {} pairs, batch {}",
        jobs.len(),
        m.personae.len(),
        active.len(),
        o.batch
    );
    // Every artifact is written after the last ballot returns, never during.
    // `briefs.json` holds other voters' briefs and `sample-ballot.md` holds a
    // full ballot; a voter that could read either mid-run would be reading
    // across the panel, which is the one thing the separation forbids.
    let sample = jobs.first().map(|j| j.prompt.clone());

    // Each voter's prompt prefix (lens, evidence base, criterion) is identical
    // across its ballots, so the backend caches it. Send one ballot per voter
    // first and the rest afterwards: the opening wave has distinct prefixes and
    // writes each cache once, instead of racing several ballots of one voter
    // against a cache none of them has warmed yet.
    let mut seen = vec![false; m.personae.len()];
    let (warm, rest): (Vec<Job>, Vec<Job>) = jobs.into_iter().partition(|j| {
        let first = !seen[j.voter];
        seen[j.voter] = true;
        first
    });

    let done = std::sync::atomic::AtomicUsize::new(0);
    let count = warm.len() + rest.len();
    let run_wave = |wave: Vec<Job>| {
        runner::pool(wave, o.concurrency, |_, job: Job| {
            let result = runner::run_oneshot(&job.model, o.timeout, &job.prompt, &voter_cwd);
            let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            eprint!("\r  {n}/{count} ballots");
            (job.voter, job.pair_ids, job.model, result)
        })
    };
    let mut outcomes = run_wave(warm);
    outcomes.extend(run_wave(rest));
    eprintln!();

    let mut votes: Vec<Vote> = Vec::new();
    let mut per_voter: Vec<Usage> = vec![Usage::default(); m.personae.len()];
    for (voter, pair_ids, model, result) in outcomes {
        let completion = match result {
            Ok(c) => c,
            Err(e) => {
                notes.push(format!("{} ballot failed ({model}): {e}", m.personae[voter].id));
                continue;
            }
        };
        total.absorb(&completion.usage);
        per_voter[voter].absorb(&completion.usage);
        let (ballots, warnings) = vote::parse_ballots(&completion.text, pair_ids.len());
        for w in warnings {
            notes.push(format!("{}: {w}", m.personae[voter].id));
        }
        for (slot, ballot) in ballots.into_iter().enumerate() {
            let Some(ballot) = ballot else { continue };
            let pair_id = pair_ids[slot];
            let (x, y) = all[pair_id];
            let a_first = pairing::a_first(pair_id, voter);
            let (shown_a, shown_b) = if a_first { (x, y) } else { (y, x) };
            let chosen = match ballot.side {
                Side::A => shown_a,
                Side::B => shown_b,
            };
            votes.push(Vote {
                pair: [m.items[x].id.clone(), m.items[y].id.clone()],
                voter: m.personae[voter].id.clone(),
                choice: m.items[chosen].id.clone(),
                confidence: ballot.confidence,
                reason: ballot.reason,
                orientation: if a_first { "ab".into() } else { "ba".into() },
            });
        }
    }

    std::fs::write(o.out.join("criterion.txt"), &criterion)?;
    if !m.dropped.is_empty() {
        std::fs::write(
            o.out.join("dropped.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "rule": o.subset, "candidates": m.dropped,
            }))?,
        )?;
    }
    if let Some(sample) = sample {
        std::fs::write(o.out.join("sample-ballot.md"), sample)?;
    }
    let briefs: Vec<_> = m
        .personae
        .iter()
        .map(|p| serde_json::json!({"id": p.id, "lens": p.lens, "brief": p.brief}))
        .collect();
    std::fs::write(
        o.out.join("briefs.json"),
        serde_json::to_string_pretty(&briefs)?,
    )?;

    let votes_path = o.out.join("votes.jsonl");
    let mut jsonl = String::new();
    for v in &votes {
        jsonl.push_str(&serde_json::to_string(v)?);
        jsonl.push('\n');
    }
    std::fs::write(&votes_path, jsonl)?;

    let known: Vec<String> = active
        .iter()
        .flat_map(|&p| {
            let (x, y) = all[p];
            [m.items[x].id.clone(), m.items[y].id.clone()]
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut report = tally::tally(&votes, Some(&known));
    report.warnings.extend(notes);

    report_usage(&m, &per_voter, &total, &o.out)?;
    emit(&report, o, &votes_path)?;
    if o.pilot.is_some() {
        eprintln!(
            "\npilot only — no full run was made. Re-run without --pilot to commit \
             to all {} pairs.",
            all.len()
        );
    }
    Ok(())
}

/// Voters are going to read this tree. Refuse to run while the things that
/// would hand them the answer are inside it.
///
/// Fatal, unlike the capability-level warning. That one rests on a list of
/// model names that will rot; this is a question about two paths, with no false
/// positives and an obvious fix. And the failure it prevents is silent — a
/// voter that found the plan returns fluent, confident ballots that merely
/// re-derive the ordering the plan already had.
///
/// `--out` is deliberately *not* required to be outside: nothing is written
/// there until the last ballot returns. A previous run's artifacts are another
/// matter, since those are on disk right now.
fn precheck_unreachable(voter_cwd: &Path, manifest: &str, o: &Opts) -> R<()> {
    let root = isolation::voter_root(voter_cwd);
    let mut found: Vec<String> = Vec::new();
    let mut check = |label: &str, path: &Path| {
        if isolation::reachable(&root, path) {
            found.push(format!("  {label:<12} {}", isolation::resolve(path).display()));
        }
    };
    check("manifest", Path::new(manifest));
    for src in &o.source {
        check("--source", Path::new(src));
    }
    // Stale votes and briefs from an earlier run are readable across the panel.
    for stale in ["votes.jsonl", "briefs.json"] {
        let path = o.out.join(stale);
        if path.exists() {
            check("previous run", &path);
        }
    }

    if !found.is_empty() {
        return Err(format!(
            "voters read this tree ({}), and these are inside it:\n{}\n\n\
             Move them outside the repository — the manifest and the plan belong \
             somewhere the voters cannot reach, and a previous run's output must \
             be cleared or redirected. Failing that, --isolate gives up repo \
             access to buy the same guarantee.",
            root.display(),
            found.join("\n")
        )
        .into());
    }

    if o.source.is_empty() {
        eprintln!(
            "panel: WARNING — no --source given, so the plan's location was not \
             checked. Voters can read {}; if the document being ranked is \
             anywhere in it, the panel is confirming it rather than judging it.",
            root.display()
        );
    } else if let Some(tracked) = tracked_in_history(&root, &o.source) {
        eprintln!(
            "panel: WARNING — {tracked} appears in this repository's git history. \
             Moving a file out of the working tree does not remove it from \
             `git log -p`, which voters can read."
        );
    }
    Ok(())
}

/// Did a source file ever live in this repository? Moving the plan out of the
/// working tree leaves it in history, and a voter can read history.
/// Best-effort: any git failure returns None rather than blocking the run.
fn tracked_in_history(root: &Path, sources: &[String]) -> Option<String> {
    for src in sources {
        let name = Path::new(src).file_name()?.to_str()?;
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["log", "--all", "--oneline", "-1", "--", &format!("*{name}")])
            .output()
            .ok()?;
        if out.status.success() && !out.stdout.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

/// Read each voter's pre-context off disk. This is the deliberate source of
/// variance between voters: different material, not different model strength.
fn load_contexts(m: &Manifest) -> R<Vec<String>> {
    let mut out = Vec::with_capacity(m.personae.len());
    for p in &m.personae {
        let mut text = String::new();
        for path in &p.context {
            let body = std::fs::read_to_string(path)
                .map_err(|e| format!("voter {}: context {path}: {e}", p.id))?;
            text.push_str(&format!("#### {path}\n{}\n\n", body.trim()));
        }
        if let Some(leak) = manifest::source_leak(&m.items, &text) {
            eprintln!("panel: WARNING — voter {}: {leak}", p.id);
        }
        if text.len() > 40_000 {
            eprintln!(
                "panel: note — voter {} carries {}KB of context. It is cached after \
                 the first ballot, but it still dilutes the pair under judgment; \
                 prefer conclusions over raw material.",
                p.id,
                text.len() / 1024
            );
        }
        out.push(text);
    }
    let sized: Vec<String> = m
        .personae
        .iter()
        .zip(&out)
        .filter(|(_, t)| !t.is_empty())
        .map(|(p, t)| format!("{} {}KB", p.id, (t.len() / 1024).max(1)))
        .collect();
    if !sized.is_empty() {
        eprintln!("context: {}", sized.join(", "));
    }
    Ok(out)
}

fn report_usage(m: &Manifest, per_voter: &[Usage], total: &Usage, out: &Path) -> R<()> {
    eprintln!();
    for (p, usage) in m.personae.iter().zip(per_voter) {
        eprintln!(
            "  {:<6} {:<22} {:>9} tok  ${:.4}",
            p.id,
            p.model.as_deref().unwrap_or("default"),
            usage.total_tokens(),
            usage.cost_usd
        );
    }
    eprintln!(
        "  {:<29} {:>9} tok  ${:.4}",
        "total",
        total.total_tokens(),
        total.cost_usd
    );
    let json = serde_json::json!({
        "total": {
            "cost_usd": total.cost_usd,
            "tokens": total.total_tokens(),
            "input_tokens": total.input_tokens,
            "output_tokens": total.output_tokens,
            "cache_read_input_tokens": total.cache_read_input_tokens,
        },
        "per_voter": m.personae.iter().zip(per_voter).map(|(p, u)| serde_json::json!({
            "id": p.id,
            "model": p.model,
            "cost_usd": u.cost_usd,
            "tokens": u.total_tokens(),
        })).collect::<Vec<_>>(),
    });
    std::fs::write(out.join("usage.json"), serde_json::to_string_pretty(&json)?)?;
    Ok(())
}

fn tally_only(path: &str, o: &Opts) -> R<()> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut votes = Vec::new();
    let mut bad = Vec::new();
    for (n, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Vote>(line) {
            Ok(v) => votes.push(v),
            Err(e) => bad.push(format!("line {}: {e}", n + 1)),
        }
    }
    if votes.is_empty() {
        return Err(format!("no usable votes in {path}").into());
    }
    let known = match &o.manifest {
        Some(p) => Some(
            Manifest::load(p)?
                .items
                .iter()
                .map(|i| i.id.clone())
                .collect::<Vec<_>>(),
        ),
        None => None,
    };
    let mut report = tally::tally(&votes, known.as_deref());
    report.warnings.extend(bad);
    emit(&report, o, Path::new(path))
}

fn emit(report: &tally::Report, o: &Opts, votes_path: &Path) -> R<()> {
    if o.as_json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!(
        "\nitems {} · pairs {}/{} · decided {} · panel {} · unanimity {}",
        report.items,
        report.pairs_found,
        report.pairs_expected,
        report.pairs_decided,
        report.panel_size,
        report
            .unanimity_rate
            .map(|r| format!("{r:.0$}", 2))
            .unwrap_or_else(|| "n/a".into())
    );
    if report.unanimity_rate.is_some_and(|r| r > 0.9) {
        println!(
            "  WARNING: unanimity > 0.9 — the personae are not individuating. You are\n\
               paying {}x for one opinion. Fix the lenses or spread the panel across\n\
               more models before trusting this order.",
            report.panel_size
        );
    }
    println!();
    for row in &report.ranking {
        println!(
            "{:>3}. {:<28} copeland {:<4} ({} votes received)",
            row.rank, row.id, row.copeland, row.votes_received
        );
    }
    if report.voters.iter().any(|v| v.concordance.is_some()) {
        println!("\nvoter concordance (agreement with the majority of the OTHERS):");
        for v in &report.voters {
            let note = match v.concordance {
                Some(c) if c > 0.95 => "  redundant — contributes no independent evidence",
                Some(c) if c < 0.55 => "  near chance — a weak voter or a genuinely orthogonal one",
                _ => "",
            };
            let silence = if v.silence_rate > 0.0 {
                format!("  evidence silent on {:.0}%", v.silence_rate * 100.0)
            } else {
                String::new()
            };
            match v.concordance {
                Some(c) => println!(
                    "  {:<6} {:.2}  ({} votes){}{}",
                    v.id, c, v.votes, silence, note
                ),
                None => println!("  {:<6}  n/a  ({} votes){}", v.id, v.votes, silence),
            }
        }
        let mean_silence = report.voters.iter().map(|v| v.silence_rate).sum::<f64>()
            / report.voters.len().max(1) as f64;
        if !report.cycles.is_empty() && mean_silence > 0.25 {
            println!(
                "\n  DEGENERATE PARLIAMENT: cycles, and voters report their evidence \
                 silent on\n  {:.0}% of pairs. The criterion never varied, but the \
                 lenses line up with the\n  item set, so each voter can only speak \
                 confidently about its own slice and\n  is effectively answering a \
                 different question. The lenses must cut ACROSS\n  the items — each \
                 voter's evidence spanning several subsystems rather than\n  owning \
                 one. Until then the cycles are an artifact of the panel, not a\n  \
                 property of the items.",
                mean_silence * 100.0
            );
        }
    }
    if !report.cycles.is_empty() {
        println!("\nINTRANSITIVE CYCLES — report these, do not smooth them over:");
        for component in &report.cycles {
            println!("  {}", component.join(" <-> "));
            for pair in &report.pairs {
                if component.contains(&pair.pair[0]) && component.contains(&pair.pair[1]) {
                    let winner = pair.winner.as_deref().unwrap_or("tie");
                    println!(
                        "    {} vs {} → {} ({}-{})",
                        pair.pair[0], pair.pair[1], winner, pair.margin[0], pair.margin[1]
                    );
                    for [voter, choice, reason] in pair.reasons.iter().take(2) {
                        if !reason.is_empty() {
                            println!("      {voter} chose {choice}: {reason}");
                        }
                    }
                }
            }
        }
    }
    for warning in &report.warnings {
        println!("\nwarning: {warning}");
    }
    println!("\nvotes: {}", votes_path.display());
    Ok(())
}
