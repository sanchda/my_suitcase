//! ralph — external autonomous loop for Claude Code.
//!
//! Each iteration is a fresh `claude -p` process fed a stable base prompt plus a
//! schema-resolved current-task brief; continuity lives in files, not context.
//! This runner adds live stream parsing, cost and wall-clock budgets, an opt-in
//! per-iteration timeout, and no-progress/thrash detection with model escalation.
//! See `docs/superpowers/specs/` for the design and `README.md` for usage. The
//! driving files (PROMPT/VISION/BACKLOG/PROGRESS) are local to the target repo.

mod backlog;
mod backlog_edit;
mod classify;
mod config;
mod context;
mod control;
mod curate;
mod git;
mod hints;
mod init;
mod judge;
mod learn;
mod notify;
mod schema;
mod state;
mod status;
mod stream;
mod supervisor;
mod synth;

/// Shared fallible-result alias.
pub type R<T> = Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "\
ralph — external autonomous loop for Claude Code (run from the repo root)

Usage: ralph [options]
       ralph init                Scaffold .ralph/ in the current repo
       ralph start [options]     Ask a running ralphd to launch the loop (writes START)
       ralph stop [options]      Ask a running loop to halt after the current task
       ralph hints               Lessons for writing a per-project PROMPT.md
       ralph schema              Explain the backlog schema and lint workflow
       ralph lint [options]      Validate backlog schema and task routing
       ralph brief [options]     Print the runner-resolved iteration brief
       ralph status [--json]     Print backlog frontier (JSON with --json)
       ralph backlog <add|edit> ...  Add or edit a backlog task (schema-checked)
       ralph learn               Mine run.log for durable lessons (propose only)
       ralph learn --apply [1,3] Write proposed learnings to .ralph/learnings/
       ralph learn --discard     Drop the saved proposals
  --prompt <file>          Prompt fed each iteration (default .ralph/PROMPT.md)
  --backlog <file>         Backlog archived on completion (default .ralph/BACKLOG.md)
  --progress <file>        Current hand-off file (default .ralph/PROGRESS.md)
  --model <name>           Default model tier (default sonnet)
  --effort <level>         auto, inherit, low, medium, high, xhigh, or max
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
  --no-yolo                Do NOT pass --dangerously-skip-permissions
  -h, --help               This help

Config-file-only settings (.ralph/ralph.toml — no flag; see README):
  synth_model, judge_tiers, judge_model, escalation_ladder,
  limit_wait[_max], transient_wait[_max], extra_args

Completion closes the arc: BACKLOG and the carry-forward are archived, PROGRESS
is cleared, and the iteration counter resets. `ralph backlog add` bootstraps a
fresh backlog when none exists, so the next arc starts from `backlog add`.
`.ralph/learnings/` persists across arcs.

Control while running:
  ralph stop                   Halt gracefully after the current iteration
  touch .ralph/STOP            Same, by hand (also suppresses --restart)
  cat .ralph/live              Live status of the active iteration
  tail -f .ralph/current.log   Watch the active iteration's raw stream

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
    if argv.first().map(String::as_str) == Some("backlog") {
        return backlog_edit::run(&argv[1..]);
    }
    if argv.first().map(String::as_str) == Some("learn") {
        return learn::run(&argv[1..]);
    }

    let command = argv.first().map(String::as_str);
    // `stop`/`start` and the inspect-only commands take flags after the subcommand.
    let subcommand = matches!(command, Some("brief" | "lint" | "stop" | "start"));
    let inspect_only = matches!(command, Some("brief" | "lint"));
    let args = if subcommand { &argv[1..] } else { &argv[..] };

    // Resolve the config path first (from flags/env), load the file, then apply
    // the full precedence chain: defaults ← file ← env ← flags.
    let mut cfg = config::load_base(args)?;
    if config::apply_args(&mut cfg, args)? {
        print!("{USAGE}");
        return Ok(0);
    }
    config::validate(&cfg)?;

    if command == Some("stop") {
        let state = state::State::open(&cfg.dir)?;
        state.request_stop()?;
        println!(
            "ralph: stop requested → {} (loop halts after the current task; suppresses --restart)",
            cfg.dir.join("STOP").display()
        );
        return Ok(0);
    }

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
            print!("{}", resolved.render());
        } else {
            print!("{}", resolved.lint_report());
        }
        return Ok(if resolved.has_errors() { 1 } else { 0 });
    }

    // Hand off to the supervisor: it forks the loop as a child and, when
    // configured, relaunches it after an ungraceful death. It runs the loop
    // inline when there's nothing to watch.
    supervisor::run(&cfg)
}
