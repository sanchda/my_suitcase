//! ralph — external autonomous loop for Claude Code.
//!
//! Each iteration is a fresh `claude -p` process fed a stable base prompt plus a
//! schema-resolved current-task brief; continuity lives in files, not context.
//! This runner adds live stream parsing, cost and wall-clock budgets, an opt-in
//! per-iteration timeout, and no-progress/thrash detection with model escalation.
//! See `docs/superpowers/specs/` for the design and `README.md` for usage. The
//! driving files (PROMPT/VISION/BACKLOG/PROGRESS) are local to the target repo.

mod backlog;
mod classify;
mod config;
mod context;
mod control;
mod git;
mod init;
mod schema;
mod state;
mod stream;

/// Shared fallible-result alias.
pub type R<T> = Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "\
ralph — external autonomous loop for Claude Code (run from the repo root)

Usage: ralph [options]
       ralph init                Scaffold .ralph/ in the current repo
       ralph schema              Explain the backlog schema and lint workflow
       ralph lint [options]      Validate backlog schema and task routing
       ralph brief [options]     Print the runner-resolved iteration brief
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
  --once                   Run a single iteration then exit (testing)
  --no-yolo                Do NOT pass --dangerously-skip-permissions
  -h, --help               This help

Control while running:
  touch .ralph/STOP            Halt gracefully after the current iteration
  cat .ralph/live              Live status of the active iteration
  tail -f .ralph/current.log   Watch the active iteration's raw stream
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

    let command = argv.first().map(String::as_str);
    let inspect_only = matches!(command, Some("brief" | "lint"));
    let args = if inspect_only { &argv[1..] } else { &argv[..] };

    // Resolve the config path first (from flags/env), load the file, then apply
    // the full precedence chain: defaults ← file ← env ← flags.
    let cpath = config::config_path(args, |k| std::env::var(k).ok());
    let mut cfg = config::Config::default();
    if cpath.exists() {
        let text = std::fs::read_to_string(&cpath)?;
        let file: config::FileConfig =
            toml::from_str(&text).map_err(|e| format!("parsing {}: {e}", cpath.display()))?;
        config::apply_file(&mut cfg, file)?;
    }
    config::apply_env(&mut cfg, |k| std::env::var(k).ok())?;
    if config::apply_args(&mut cfg, args)? {
        print!("{USAGE}");
        return Ok(0);
    }
    config::validate(&cfg)?;

    if inspect_only {
        let resolved = context::load(&cfg.backlog, &cfg.progress);
        if command == Some("brief") {
            print!("{}", resolved.render());
        } else {
            print!("{}", resolved.lint_report());
        }
        return Ok(if resolved.has_errors() { 1 } else { 0 });
    }

    control::run(&cfg)
}
