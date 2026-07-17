//! ralph — external autonomous loop for Claude Code.
//!
//! Each iteration is a fresh `claude -p` process fed the same prompt; continuity
//! lives in files, not context. This runner adds live stream parsing, cost and
//! wall-clock budgets, an opt-in per-iteration timeout, and no-progress/thrash
//! detection with model escalation. See `docs/superpowers/specs/` for the design
//! and `README.md` for usage. The driving files (PROMPT/VISION/BACKLOG/PROGRESS)
//! are local to whatever repo you run this in.

mod classify;
mod config;
mod control;
mod git;
mod state;
mod stream;

/// Shared fallible-result alias.
pub type R<T> = Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "\
ralph — external autonomous loop for Claude Code (run from the repo root)

Usage: ralph [options]
  --prompt <file>          Prompt fed each iteration (default tools/ralph/PROMPT.md)
  --model <name>           Default model tier (default sonnet)
  --fallback-model <name>  Overloaded-fallback model (\"\" disables)
  --max-iterations <n>     Stop after n iterations (0 = unlimited)
  --max-cost <usd>         Stop once cumulative cost reaches this (0 = off)
  --max-duration <dur>     Stop after this wall-clock time, e.g. 8h/30m/300s (0 = off)
  --iteration-timeout <dur> Kill an iteration that runs longer than this (0 = off)
  --escalate-after <n>     No-progress streak before escalating the model (default 2)
  --abort-after <n>        No-progress streak before aborting (default 4)
  --marker <text>          Completion token (default RALPH_COMPLETE)
  --dir <path>             Runtime/log dir (default .ralph)
  --config <file>          Config file (default tools/ralph/ralph.toml)
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

    // Resolve the config path first (from flags/env), load the file, then apply
    // the full precedence chain: defaults ← file ← env ← flags.
    let cpath = config::config_path(&argv, |k| std::env::var(k).ok());
    let mut cfg = config::Config::default();
    if cpath.exists() {
        let text = std::fs::read_to_string(&cpath)?;
        let file: config::FileConfig = toml::from_str(&text)
            .map_err(|e| format!("parsing {}: {e}", cpath.display()))?;
        config::apply_file(&mut cfg, file)?;
    }
    config::apply_env(&mut cfg, |k| std::env::var(k).ok())?;
    if config::apply_args(&mut cfg, &argv)? {
        print!("{USAGE}");
        return Ok(0);
    }
    config::validate(&cfg)?;

    control::run(&cfg)
}
