//! The control loop: spawn each iteration, consume its stream, classify the
//! outcome, apply backoff, and drive no-progress escalation and budgets.
//!
//! The thrash tracker ([`Thrash`]) is a pure state machine tested in isolation;
//! the loop wires it to real subprocesses, git, and the runtime dir.

use crate::classify::{classify, Class};
use crate::config::Config;
use crate::state::State;
use crate::stream::{self, IterStatus, ResultEnvelope};
use crate::{git, R};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// What an iteration achieved, from the thrash tracker's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// A code iteration that advanced HEAD — real progress.
    Made,
    /// A declared non-code pass (review/blocked/…): excluded from the streak.
    Excluded,
    /// No progress: code iteration with no commit, or a transient/timeout retry.
    NoProgress,
}

/// What the loop should do next, after recording a verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Continue,
    /// Force this model on the next attempt (escalation).
    Escalate(String),
    /// Halt: no progress after too many iterations.
    Abort(String),
}

/// Pure no-progress tracker: counts consecutive unproductive iterations and
/// decides when to escalate the model tier and when to give up.
pub struct Thrash {
    escalate_after: u32,
    abort_after: u32,
    ladder: Vec<String>,
    streak: u32,
    escalation_idx: Option<usize>,
}

impl Thrash {
    pub fn new(cfg: &Config) -> Self {
        Thrash {
            escalate_after: cfg.escalate_after,
            abort_after: cfg.abort_after,
            ladder: cfg.escalation_ladder.clone(),
            streak: 0,
            escalation_idx: None,
        }
    }

    /// The model currently forced by escalation, if any.
    pub fn forced_model(&self) -> Option<String> {
        self.escalation_idx.map(|i| self.ladder[i].clone())
    }

    /// Record an iteration's verdict. `resolved_model` is the tier that ran, used
    /// to compute the next escalation step.
    pub fn record(&mut self, v: Verdict, resolved_model: &str) -> Action {
        match v {
            Verdict::Made => {
                self.streak = 0;
                self.escalation_idx = None;
                Action::Continue
            }
            Verdict::Excluded => Action::Continue,
            Verdict::NoProgress => {
                self.streak += 1;
                if self.streak >= self.abort_after {
                    let top = self.forced_model().unwrap_or_else(|| resolved_model.to_string());
                    return Action::Abort(format!(
                        "no progress after {} iterations (escalated to {top})",
                        self.streak
                    ));
                }
                if self.streak >= self.escalate_after {
                    let cur = self
                        .escalation_idx
                        .or_else(|| self.ladder.iter().position(|m| m == resolved_model))
                        .unwrap_or(0);
                    let next = (cur + 1).min(self.ladder.len() - 1);
                    self.escalation_idx = Some(next);
                    return Action::Escalate(self.ladder[next].clone());
                }
                Action::Continue
            }
        }
    }
}

/// Capped exponential backoff: 0 → base, else min(cur*2, cap).
pub fn next_backoff(cur: u64, base: u64, cap: u64) -> u64 {
    let n = if cur == 0 { base } else { cur.saturating_mul(2) };
    n.min(cap)
}

/// Result of running a single iteration.
struct Ran {
    envelope: Option<ResultEnvelope>,
    killed: bool,
}

/// Run the whole loop. Returns the process exit code.
pub fn run(cfg: &Config) -> R<i32> {
    if which("claude").is_none() {
        return Err("claude CLI not found on PATH".into());
    }
    if !cfg.prompt.exists() {
        return Err(format!("prompt file not found: {}", cfg.prompt.display()).into());
    }
    let state = State::open(&cfg.dir)?;
    let repo = Path::new(".");
    git::write_baseline(repo, &state.baseline_path());

    state.log(&format!(
        "=== ralph start (model={} fallback={} marker={} max_iter={} max_cost={} max_dur={}s yolo={}) ===",
        cfg.model,
        if cfg.fallback_model.is_empty() { "none" } else { &cfg.fallback_model },
        cfg.marker,
        cfg.max_iterations,
        cfg.max_cost_usd,
        cfg.max_duration,
        cfg.yolo,
    ));

    let mut thrash = Thrash::new(cfg);
    let mut iter = state.iteration();
    let mut lwait = 0u64;
    let mut twait = 0u64;
    let mut cost_total = 0.0f64;
    let start = Instant::now();

    loop {
        // --- boundary checks ---
        if state.stop_requested() {
            state.log("STOP file present → halting");
            state.clear_stop();
            break;
        }
        if cfg.max_iterations > 0 && iter >= cfg.max_iterations {
            state.log(&format!("max iterations ({}) reached → halting", cfg.max_iterations));
            break;
        }
        if cfg.max_cost_usd > 0.0 && cost_total >= cfg.max_cost_usd {
            state.log(&format!(
                "cost budget reached (${:.4} ≥ ${:.4}) → halting",
                cost_total, cfg.max_cost_usd
            ));
            break;
        }
        if cfg.max_duration > 0 && start.elapsed().as_secs() >= cfg.max_duration {
            state.log(&format!("wall-clock budget ({}s) reached → halting", cfg.max_duration));
            break;
        }

        let next = iter + 1;
        let model = thrash
            .forced_model()
            .or_else(|| state.read_model(&cfg.escalation_ladder))
            .unwrap_or_else(|| cfg.model.clone());
        let head_before = git::head(repo);

        state.log(&format!("iter {next} → {model}"));
        let ran = run_one(cfg, &state, next, &model)?;

        // --- interpret outcome ---
        let (class, cost, text) = match &ran.envelope {
            Some(env) => {
                state.write_last_result(&env.raw);
                let c = classify(env.is_error, env.api_error_status, &env.result);
                (c, env.total_cost_usd, env.result.clone())
            }
            // No envelope: crash, kill, or empty output → transient.
            None => (Class::Transient, 0.0, String::new()),
        };
        cost_total += cost;

        match class {
            Class::Success => {
                iter = next;
                state.set_iteration(iter)?;
                lwait = 0;
                twait = 0;
                let snippet: String = text.chars().take(160).collect();
                state.log(&format!("  ok (${cost:.4}) — {}", snippet.replace('\n', " ")));

                if stream::has_marker(&text, &cfg.marker) {
                    state.log("  marker seen (own line) → COMPLETE");
                    state.log(&format!("=== ralph COMPLETE after {iter} iterations ==="));
                    break;
                }

                // Classify the iteration for thrash tracking.
                let status = state.read_status();
                let verdict = match status.as_deref() {
                    Some(s) if s != "code" => {
                        state.log(&format!("  · non-code pass ({s}) — excluded from progress streak"));
                        Verdict::Excluded
                    }
                    _ => {
                        if git::advanced_since(repo, &head_before) {
                            Verdict::Made
                        } else {
                            state.log("  ⚠ code iteration with no new commit — counts as no-progress");
                            Verdict::NoProgress
                        }
                    }
                };
                newly_dirty_warn(&state, repo);
                state.clear_status();
                if apply_verdict(&mut thrash, verdict, &model, &state) {
                    return Ok(1);
                }
                if cfg.once {
                    state.log("--once → stop");
                    break;
                }
            }
            Class::Limit => {
                // Pure wait — never feeds thrash; unlimited retries.
                let snippet: String = text.chars().take(160).collect();
                state.log(&format!("  USAGE/RATE LIMIT — {}", snippet.replace('\n', " ")));
                lwait = next_backoff(lwait, cfg.limit_wait, cfg.limit_wait_max);
                state.log(&format!("  limit backoff: sleeping {lwait}s, then retry iter {next}"));
                thread::sleep(Duration::from_secs(lwait));
            }
            Class::Transient => {
                let reason = if ran.killed {
                    "killed by per-iteration timeout".to_string()
                } else {
                    let snippet: String = text.chars().take(160).collect();
                    format!("transient — {}", snippet.replace('\n', " "))
                };
                state.log(&format!("  {reason}"));
                // A transient (including a timeout strike) is no-progress.
                if apply_verdict(&mut thrash, Verdict::NoProgress, &model, &state) {
                    return Ok(1);
                }
                twait = next_backoff(twait, cfg.transient_wait, cfg.transient_wait_max);
                state.log(&format!("  transient backoff: sleeping {twait}s, then retry iter {next}"));
                thread::sleep(Duration::from_secs(twait));
            }
            Class::Fatal => {
                let snippet: String = text.chars().take(200).collect();
                state.log(&format!("=== ralph ABORTED (fatal) — {} ===", snippet.replace('\n', " ")));
                return Ok(1);
            }
        }
    }
    Ok(0)
}

/// Apply a verdict to the tracker, logging escalation and returning `true` if the
/// loop should abort.
fn apply_verdict(thrash: &mut Thrash, v: Verdict, model: &str, state: &State) -> bool {
    match thrash.record(v, model) {
        Action::Continue => false,
        Action::Escalate(m) => {
            state.log(&format!("  ↑ no-progress streak → escalating model to {m}"));
            false
        }
        Action::Abort(reason) => {
            state.log(&format!("=== ralph ABORTED — {reason} ==="));
            true
        }
    }
}

/// Warn (don't act) if the tracked tree gained new dirt vs. the baseline.
fn newly_dirty_warn(state: &State, repo: &Path) {
    let n = git::newly_dirty(repo, &state.baseline_path());
    if n > 0 {
        state.log(&format!("  ⚠ {n} newly-dirty tracked file(s) — agent may have skipped its commit"));
    }
}

/// Spawn and drive one `claude` iteration.
fn run_one(cfg: &Config, state: &State, n: u64, model: &str) -> R<Ran> {
    let log_path = state.new_iter_log(n)?;

    let mut args: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        cfg.output_format.clone(),
    ];
    if cfg.output_format == "stream-json" {
        args.push("--verbose".into());
    }
    if cfg.yolo {
        args.push("--dangerously-skip-permissions".into());
    }
    args.push("--model".into());
    args.push(model.to_string());
    let fb = &cfg.fallback_model;
    if !fb.is_empty() && fb != model {
        args.push("--fallback-model".into());
        args.push(fb.clone());
    }
    args.extend(cfg.extra_args.iter().cloned());

    let prompt = File::open(&cfg.prompt)?;
    let mut cmd = Command::new("claude");
    cmd.args(&args)
        .stdin(Stdio::from(prompt))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // With a per-iteration timeout, run the child in its own process group so
    // the watchdog can kill the WHOLE tree (claude + its tool subprocesses) —
    // otherwise a killed leader's children keep the stdout pipe open and the
    // hung iteration isn't reclaimed. Only when a timeout is set: an isolated
    // group would otherwise stop Ctrl-C from propagating to the child.
    #[cfg(unix)]
    if cfg.iteration_timeout > 0 {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;
    let pid = child.id();

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // Drain stderr into the same raw log (best-effort) on its own thread.
    let stderr_log = log_path.clone();
    let stderr_thread = thread::spawn(move || {
        use std::io::{BufRead, Write};
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&stderr_log) {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = writeln!(f, "{line}");
            }
        }
    });

    // Watchdog: kill the child's process group if it outlives the timeout.
    let killed = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let watchdog = if cfg.iteration_timeout > 0 {
        let (killed_w, done_w) = (killed.clone(), done.clone());
        let deadline = cfg.iteration_timeout;
        Some(thread::spawn(move || {
            let ticks = deadline * 10; // poll every 100ms
            for _ in 0..ticks {
                if done_w.load(Ordering::SeqCst) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
            if !done_w.load(Ordering::SeqCst) {
                killed_w.store(true, Ordering::SeqCst);
                kill_group(pid);
            }
        }))
    } else {
        None
    };

    // Consume the stream on this thread (blocks until EOF / child exit / kill).
    let mut raw = std::fs::OpenOptions::new().append(true).open(&log_path)?;
    let mut status = IterStatus::new(n, model);
    state.write_live_status(&status.render());
    let reader = BufReader::new(stdout);
    let envelope = stream::consume(reader, &mut raw, &mut status, |s| {
        state.write_live_status(&s.render());
    })?;

    // Signal watchdog to stop, reap the child and the stderr drainer.
    done.store(true, Ordering::SeqCst);
    let _ = child.wait();
    let _ = stderr_thread.join();
    if let Some(w) = watchdog {
        let _ = w.join();
    }

    let killed = killed.load(Ordering::SeqCst);
    let envelope = if killed { None } else { envelope };
    state.write_live_status(&format!("iter {n} finished (killed={killed})\n"));
    Ok(Ran { envelope, killed })
}

/// Kill the process group led by `pid` with SIGKILL. The child is spawned as
/// its own group leader (see `run_one`), so the negative-pid target reaps
/// `claude` and every subprocess it started — reclaiming a truly hung iteration.
fn kill_group(pid: u32) {
    let _ = Command::new("kill").arg("-9").arg(format!("-{pid}")).status();
}

/// Minimal PATH lookup for a program (avoids a `which` dependency).
fn which(prog: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(prog);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(escalate: u32, abort: u32) -> Config {
        Config { escalate_after: escalate, abort_after: abort, ..Config::default() }
    }

    #[test]
    fn backoff_caps() {
        assert_eq!(next_backoff(0, 300, 3600), 300);
        assert_eq!(next_backoff(300, 300, 3600), 600);
        assert_eq!(next_backoff(2000, 300, 3600), 3600); // capped
        assert_eq!(next_backoff(0, 10, 300), 10);
    }

    #[test]
    fn made_resets_streak_and_escalation() {
        let mut t = Thrash::new(&cfg(2, 4));
        assert_eq!(t.record(Verdict::NoProgress, "sonnet"), Action::Continue); // streak 1
        assert_eq!(t.record(Verdict::NoProgress, "sonnet"), Action::Escalate("opus".into())); // streak 2
        assert_eq!(t.forced_model(), Some("opus".into()));
        assert_eq!(t.record(Verdict::Made, "opus"), Action::Continue);
        assert_eq!(t.forced_model(), None);
        assert_eq!(t.streak, 0);
    }

    #[test]
    fn escalates_up_the_ladder_then_aborts() {
        let mut t = Thrash::new(&cfg(2, 4));
        assert_eq!(t.record(Verdict::NoProgress, "haiku"), Action::Continue); // 1
        // streak 2 → escalate one tier above the running model (haiku → sonnet)
        assert_eq!(t.record(Verdict::NoProgress, "haiku"), Action::Escalate("sonnet".into()));
        // streak 3 → escalate again (sonnet → opus), computed from forced idx
        assert_eq!(t.record(Verdict::NoProgress, "sonnet"), Action::Escalate("opus".into()));
        // streak 4 → abort
        match t.record(Verdict::NoProgress, "opus") {
            Action::Abort(msg) => assert!(msg.contains("opus")),
            other => panic!("expected abort, got {other:?}"),
        }
    }

    #[test]
    fn excluded_passes_do_not_move_streak() {
        let mut t = Thrash::new(&cfg(2, 4));
        assert_eq!(t.record(Verdict::NoProgress, "sonnet"), Action::Continue); // 1
        assert_eq!(t.record(Verdict::Excluded, "sonnet"), Action::Continue); // still 1
        assert_eq!(t.streak, 1);
        assert_eq!(t.record(Verdict::NoProgress, "sonnet"), Action::Escalate("opus".into())); // 2
    }

    #[test]
    fn escalation_clamps_at_top() {
        let mut t = Thrash::new(&cfg(1, 9));
        // Already at opus; escalation can't go higher.
        assert_eq!(t.record(Verdict::NoProgress, "opus"), Action::Escalate("opus".into()));
        assert_eq!(t.record(Verdict::NoProgress, "opus"), Action::Escalate("opus".into()));
    }
}
