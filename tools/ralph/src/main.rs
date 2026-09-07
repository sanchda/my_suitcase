//! ralph — external autonomous loop for Claude Code and Codex.
//!
//! Each iteration is a fresh `claude -p` process fed a stable base prompt plus a
//! schema-resolved current-task brief; continuity lives in files, not context.
//! This runner adds live stream parsing, cost and wall-clock budgets, an opt-in
//! per-iteration timeout, and no-progress/thrash detection with model escalation.
//! See `docs/superpowers/specs/` for the design and `README.md` for usage. The
//! driving files (PROMPT/VISION/BACKLOG/PROGRESS) are local to the target repo.

mod acceptance;
mod backend;
mod backlog;
mod backlog_cli;
mod backlog_edit;
mod classify;
mod config;
mod context;
mod control;
mod curate;
mod doctor;
mod git;
mod hints;
mod inbox;
mod init;
mod judge;
mod learn;
mod ledger;
mod limits;
mod model;
mod msg;
mod notify;
mod pidguard;
mod runtime;
mod schema;
mod state;
mod status;
mod stop;
mod stream;
mod supervisor;
mod synth;

/// Shared fallible-result alias.
pub type R<T> = Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "\
ralph — external autonomous loop for Claude Code and Codex (run from the repo root)

Usage: ralph [options]             Run the loop here until the backlog completes

Setup and lifecycle
  ralph init                       Scaffold .ralph/ in the current repo
  ralph start [options]            Ask a running ralphd to launch the loop
  ralph stop [--async] [--force]    Wait for a graceful stop; --force halts now

Inspect
  ralph status [--json]            Backlog frontier: iteration, current, upcoming
  ralph lint [options]             Validate backlog schema and task routing
  ralph brief [--full] [options]   Resolved brief, or the exact composed prompt
  ralph doctor [--json] [options]  Check setup without calling models or tests

Backlog — schema-checked, and queued while a loop runs (see below)
  ralph add [<id>] <title> [--verify <cmd>]
                                   Queue a task; <id> places a child, e.g. 3.1.1
  ralph add --under <parent> <title>
                                   Queue the next free <parent>.N stage
  ralph done <id>                  Queue a check-off
  ralph uncheck <id>               Queue a reopen
  ralph drop <id> [--recursive]    Queue a removal (archived, never deleted)
  ralph backlog <add|edit> ...     Older flag-style forms, kept as aliases

Steer a running loop
  ralph model <name>               One-shot model override for the next iteration
  ralph msg [--new] <text>         Talk to this loop's persistent agent session

Learn
  ralph learn                      Mine run.log for durable lessons (propose only)
  ralph learn --apply [1,3]        Write proposed learnings to .ralph/learnings/
  ralph learn --discard            Drop the saved proposals

Reference
  ralph hints                      Lessons for writing a per-project PROMPT.md
  ralph schema                     The backlog schema and lint workflow

Options
  --prompt <file>          Prompt fed each iteration (default .ralph/PROMPT.md)
  --backlog <file>         Backlog archived on completion (default .ralph/BACKLOG.md)
  --progress <file>        Current hand-off file (default .ralph/PROGRESS.md)
  --model, -m <name>       Default tier or concrete model (default sonnet)
  --backend <name>         auto (default), claude, or codex (alias openai)
  --synth-model <name>     Carry-forward / learning model
  --judge-model <name>     Adversarial judge model
  --effort <level>         auto, inherit, low, medium, high, xhigh, or max
  --provider-failover <bool> Switch providers on depleted usage (default true)
  --failover-cooldown <dur> Fallback wait when reset time is unknown (default 30m)
  --fallback-model <name>  Overloaded-fallback model (\"\" disables)
  --max-iterations <n>     Stop after n iterations (0 = unlimited)
  --max-cost <usd>         Stop once cumulative cost reaches this (0 = off)
  --max-duration <dur>     Stop after this wall-clock time, e.g. 8h/30m/300s (0 = off)
  --iteration-timeout <dur> Kill an iteration that runs longer than this (0 = off)
  --escalate-after <n>     No-progress streak before escalating the model (default 2)
  --abort-after <n>        No-progress streak before aborting (default 4)
  --marker <text>          Completion token (default RALPH_COMPLETE)
  --dir <path>             Runtime/log dir (default .ralph)
  --config <file>          Config file (default .ralph/ralph.toml)
  --restart <bool>         Relaunch the loop after an ungraceful death (default false)
  --heartbeat <dur>        Post live per-iteration status this often (0 = off)
  --once                   Run a single iteration then exit (testing)
  --no-yolo                Use the backend's normal permissions (Codex: workspace-write)
  -h, --help               This help

Config-file-only settings (.ralph/ralph.toml — no flag; see README):
  tier_models, failover_models, judge_tiers, escalation_ladder,
  limit_wait[_max], transient_wait[_max], extra_args,
  budget_usd, budget_window, acceptance, task_attempt_limit

Backlog mutations never write BACKLOG.md in place. While a loop is running they
queue to .ralph/inbox/ and apply at the next iteration boundary — so `add` and
friends print `queued …` and the file does not change yet. That is success, not
a failure to apply; it is what keeps the backlog from shifting under a running
agent. With no loop running they apply immediately.

Completion closes the arc: BACKLOG and the carry-forward are archived, PROGRESS
is cleared, and the iteration counter resets. `ralph add` bootstraps a fresh
backlog when none exists, so the next arc starts from `add`.
`.ralph/learnings/` persists across arcs.

Watching a running loop:
  cat .ralph/live              Live status of the active iteration
  tail -f .ralph/current.log   Watch the active iteration's raw stream
  tail -f .ralph/run.log       One line per iteration, plus perf and warnings

One loop per repo: the loop holds .ralph/loop.pid and a second `ralph` here
exits 2 naming the live pid. A pidfile left by a killed loop is reclaimed on
the next start. Full documentation in tools/ralph/README.md.

With --restart, only an ungraceful death (killed by a signal: OOM, kill, crash)
relaunches the loop; graceful halts, completion, and abort are terminal, and a
pending STOP suppresses the restart.
";

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("ralph: {e}");
            std::process::exit(2);
        }
    }
}

fn run() -> R<i32> {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    if argv.first().map(String::as_str) == Some("init") {
        return init::run();
    }
    if argv.first().map(String::as_str) == Some("schema") {
        return schema::run(&argv[1..]);
    }
    if argv.first().map(String::as_str) == Some("hints") {
        return hints::run(&argv[1..]);
    }
    if argv.first().map(String::as_str) == Some("status") {
        return status::run(&argv[1..]);
    }
    if argv.first().map(String::as_str) == Some("doctor") {
        return doctor::run(&argv[1..]);
    }
    if argv.first().map(String::as_str) == Some("backlog") {
        return backlog_edit::run(&argv[1..]);
    }
    if argv.first().map(String::as_str) == Some("learn") {
        return learn::run(&argv[1..]);
    }
    if argv.first().map(String::as_str) == Some("model") {
        return model::run(&argv[1..]);
    }
    if argv.first().map(String::as_str) == Some("msg") {
        return msg::run(&argv[1..]);
    }
    if let Some(sub @ ("add" | "drop" | "done" | "uncheck")) = argv.first().map(String::as_str) {
        return backlog_cli::run(sub, &argv[1..]);
    }
    // `stop` resolves its own config: `--now` would not survive `apply_args`.
    if argv.first().map(String::as_str) == Some("stop") {
        return stop::run(&argv[1..]);
    }

    let command = argv.first().map(String::as_str);
    // `start` and the inspect-only commands take flags after the subcommand.
    let subcommand = matches!(command, Some("brief" | "lint" | "start"));
    let inspect_only = matches!(command, Some("brief" | "lint"));
    let args = if subcommand { &argv[1..] } else { &argv[..] };
    let full = command == Some("brief") && args.iter().any(|a| a == "--full");
    let filtered = args
        .iter()
        .filter(|a| !full || a.as_str() != "--full")
        .cloned()
        .collect::<Vec<_>>();
    let args = filtered.as_slice();

    // Resolve the config path first (from flags/env), load the file, then apply
    // the full precedence chain: defaults ← file ← env ← flags.
    let mut cfg = config::load_base(args)?;
    if config::apply_args(&mut cfg, args)? {
        print!("{USAGE}");
        return Ok(0);
    }
    config::validate(&cfg)?;

    if command == Some("start") {
        let state = state::State::open(&cfg.dir)?;
        state.request_start()?;
        println!(
            "ralph: start requested → {} (a running ralphd will launch the loop)",
            cfg.dir.join("START").display()
        );
        return Ok(0);
    }

    if inspect_only {
        let resolved = context::load(&cfg.backlog, &cfg.progress);
        if command == Some("brief") {
            if full {
                let prompt = context::full_prompt(&cfg, &resolved)?;
                eprintln!("ralph: full prompt {} bytes; task {}; carry-forward capped at {} bytes; leaf excerpt 8192 bytes, ancestors 4096 bytes total", prompt.len(), resolved.task_id.as_deref().unwrap_or("@complete"), synth::MAX_CARRY_FORWARD_BYTES);
                for warning in resolved.warnings() {
                    eprintln!("ralph: {warning}");
                }
                print!("{prompt}");
            } else {
                print!("{}", resolved.render());
            }
        } else {
            print!("{}", resolved.lint_report());
        }
        return Ok(if resolved.has_errors() { 1 } else { 0 });
    }

    supervisor::run(&cfg)
}
