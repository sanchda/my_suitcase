//! The supervisor: a non-detached parent that owns the loop's lifecycle.
//!
//! `main` hands control here; the supervisor forks a child that runs
//! [`crate::control::run`] (the real loop) and `waitpid`s on it. Because all loop
//! state lives in `.ralph/` files, a fresh child re-opens `State` and resumes at
//! the persisted iteration — so we `fork` (cheap, inherits the parsed `Config`)
//! rather than re-exec.
//!
//! The parent only ever waits, so it stays single-threaded and every `fork`
//! happens from a single-threaded process. On the child's *ungraceful* death
//! (killed by a signal — SIGKILL/OOM, SIGSEGV) the parent reports it and may
//! relaunch. Graceful exits (and a Rust panic, which unwinds to exit 101) are
//! `WIFEXITED` and terminal — their code is propagated unchanged.
//!
//! Four things independently veto a restart, so `--restart` alone does not mean
//! "always come back": a pending STOP, a deliberate signal (SIGINT/TERM/HUP/QUIT
//! — see `is_terminating_signal`), too many rapid failures (`RestartGuard`), and
//! `--restart` being unset at all. Each is enforced at its own definition.

use crate::config::Config;
use crate::notify;
use crate::state::State;
use crate::{control, pidguard, R};
use std::io::{self, Write};
use std::os::raw::c_int;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Seconds to wait before relaunching after an ungraceful death.
const RESTART_BACKOFF_SECS: u64 = 10;
/// A child that dies faster than this counts as a "rapid" failure.
const MIN_HEALTHY_SECS: u64 = 60;
/// Consecutive rapid failures before the supervisor gives up on restarting.
const MAX_RAPID_RESTARTS: u32 = 5;

/// The loop child, for the SIGTERM handler; 0 while no child is running.
static CHILD_PID: AtomicU32 = AtomicU32::new(0);

/// The single-loop-per-repo pidfile. Holds the *supervisor's* pid: it is the
/// process a `ralph stop --now` has to reach.
pub fn pidfile(dir: &Path) -> PathBuf {
    dir.join("loop.pid")
}

/// Install `handler` for `sig`. No `SA_RESTART`: `wait_for` already retries
/// across EINTR, and the other blocking waits are better off interrupted.
pub fn install_handler(sig: c_int, handler: extern "C" fn(c_int)) {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as libc::sighandler_t;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(sig, &sa, std::ptr::null_mut());
    }
}

/// Forward a stop signal to the loop child and return, so the death arrives
/// through the usual `waitpid` path and is classified there.
extern "C" fn forward_to_child(sig: c_int) {
    let pid = CHILD_PID.load(Ordering::SeqCst);
    // pid-to-pid: a hand-launched ralph does not lead its process group, so a
    // negative target would signal the invoking shell's group.
    if pid != 0 {
        unsafe { libc::kill(pid as libc::pid_t, sig) };
    }
}

/// Interpreted `waitpid` status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    /// Normal `exit(code)` — graceful, or a panic (unwinds to 101). Terminal.
    Exited(i32),
    /// Killed by `signal` — the ungraceful case a restart may replace.
    Signalled(c_int),
}

/// What the supervisor does after a signal death.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    Restart,
    /// Do not restart; exit with this code (`128 + signal`, the shell convention).
    Stop(i32),
}

/// Tracks rapid crash-looping so a child that dies instantly forever can't spin.
struct RestartGuard {
    rapid_failures: u32,
}

impl RestartGuard {
    fn new() -> Self {
        RestartGuard { rapid_failures: 0 }
    }

    /// Record a signal death and its child lifetime. Returns `true` if the
    /// supervisor may still restart, `false` once too many rapid failures pile up.
    /// A child that lived at least `MIN_HEALTHY_SECS` resets the streak.
    fn record(&mut self, lived: Duration) -> bool {
        if lived.as_secs() < MIN_HEALTHY_SECS {
            self.rapid_failures += 1;
        } else {
            self.rapid_failures = 0;
        }
        self.rapid_failures < MAX_RAPID_RESTARTS
    }
}

/// Signals that mean "deliberately stop this" — a targeted `kill`/Ctrl-C — as
/// opposed to a crash or the OOM killer. We never restart on these, so an
/// operator can always stop the loop with a signal even when `--restart` is on.
fn is_terminating_signal(sig: c_int) -> bool {
    matches!(
        sig,
        libc::SIGINT | libc::SIGTERM | libc::SIGHUP | libc::SIGQUIT
    )
}

/// Decide what to do after a signal death (pure). `guard_ok` is the crash-loop
/// guard's verdict, already computed only when a restart is otherwise eligible.
fn restart_decision(sig: c_int, restart_enabled: bool, stop_present: bool, guard_ok: bool) -> Next {
    if restart_enabled && !stop_present && guard_ok && !is_terminating_signal(sig) {
        Next::Restart
    } else {
        Next::Stop(128 + sig)
    }
}

/// Turn a raw `waitpid` status into a [`Wait`].
fn interpret(status: c_int) -> Wait {
    if libc::WIFEXITED(status) {
        Wait::Exited(libc::WEXITSTATUS(status))
    } else if libc::WIFSIGNALED(status) {
        Wait::Signalled(libc::WTERMSIG(status))
    } else {
        // Stopped/continued: not possible without WUNTRACED/WCONTINUED. Treat as
        // a clean exit so we neither restart nor hang.
        Wait::Exited(0)
    }
}

/// `waitpid` for a specific child, retrying across `EINTR`.
fn wait_for(pid: libc::pid_t) -> R<c_int> {
    loop {
        let mut status: c_int = 0;
        let r = unsafe { libc::waitpid(pid, &mut status, 0) };
        if r == pid {
            return Ok(status);
        }
        if r < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(format!("waitpid on {pid}: {err}").into());
        }
        // r == 0 only happens with WNOHANG, which we don't pass; retry defensively.
    }
}

/// Post the ungraceful-death notice.
fn report_death(notifier: &Option<notify::Notifier>, pid: libc::pid_t, sig: c_int, iter: u64) {
    let tail = if iter > 0 {
        format!(", last iter {iter}")
    } else {
        String::new()
    };
    notify::notify(
        notifier,
        &format!(
            "💀 **ralph terminated** — pid {pid} killed by signal {sig} \
             (OOM, kill, or crash){tail}."
        ),
    );
}

/// Run the loop under supervision. Returns the process exit code.
pub fn run(cfg: &Config) -> R<i32> {
    let path = pidfile(&cfg.dir);
    // Taken before either path below, and released by its Drop on a graceful exit.
    let _pidfile = match pidguard::acquire(&path) {
        Ok(guard) => guard,
        Err(Some(pid)) => {
            return Err(format!("a loop is already running in this repo (pid {pid})").into())
        }
        Err(None) => return Err(format!("cannot take {}", path.display()).into()),
    };

    // Nothing to supervise (no restart, no webhook to report a death on) → run the
    // loop inline and skip the fork entirely.
    if !cfg.restart && cfg.discord_webhook.trim().is_empty() {
        return control::run(cfg);
    }

    install_handler(libc::SIGTERM, forward_to_child);
    let notifier = notify::Notifier::new(&cfg.discord_webhook);
    let mut guard = RestartGuard::new();

    loop {
        // Flush before forking so buffered parent output isn't duplicated by the child.
        let _ = io::stdout().flush();
        let start = Instant::now();

        // SAFETY: the supervisor never spawns threads (it only waits), so the
        // process is single-threaded at every fork — no locked-mutex inheritance.
        match unsafe { libc::fork() } {
            -1 => return Err(format!("fork: {}", io::Error::last_os_error()).into()),
            0 => {
                // Child: run the loop, then exit with its code. Never returns here.
                let code = match control::run(cfg) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("ralph: {e}");
                        2
                    }
                };
                let _ = io::stdout().flush();
                std::process::exit(code);
            }
            pid => {
                CHILD_PID.store(pid as u32, Ordering::SeqCst);
                let status = wait_for(pid)?;
                CHILD_PID.store(0, Ordering::SeqCst);
                let lived = start.elapsed();
                match interpret(status) {
                    Wait::Exited(code) => return Ok(code),
                    Wait::Signalled(sig) => {
                        let state = State::open(&cfg.dir).ok();
                        let iter = state.as_ref().map(State::iteration).unwrap_or(0);
                        report_death(&notifier, pid, sig, iter);

                        let stop = state.as_ref().map(State::stop_requested).unwrap_or(false);
                        // Only consult (and mutate) the crash-loop guard when a
                        // restart is otherwise eligible.
                        let eligible = cfg.restart && !stop;
                        let guard_ok = eligible && guard.record(lived);

                        match restart_decision(sig, cfg.restart, stop, guard_ok) {
                            Next::Restart => {
                                if let Some(s) = &state {
                                    s.log(&format!(
                                        "supervisor: ungraceful death (signal {sig}, child lived {}s) → restarting in {RESTART_BACKOFF_SECS}s",
                                        lived.as_secs()
                                    ));
                                }
                                notify::notify(
                                    &notifier,
                                    &format!(
                                        "🔁 **ralph restarting** — relaunching from iter {iter} after ungraceful death (signal {sig})"
                                    ),
                                );
                                thread::sleep(Duration::from_secs(RESTART_BACKOFF_SECS));
                                continue;
                            }
                            Next::Stop(code) => {
                                if let Some(s) = &state {
                                    if stop {
                                        s.clear_stop();
                                        s.log("supervisor: STOP present → not restarting");
                                    } else if is_terminating_signal(sig) {
                                        // A bare `kill -TERM` is not a crash loop.
                                        s.log(&format!(
                                            "supervisor: deliberate termination (signal {sig}) → not restarting"
                                        ));
                                    } else if cfg.restart {
                                        s.log("supervisor: too many rapid crashes → giving up on restart");
                                        notify::notify(
                                            &notifier,
                                            "🔴 **ralph** — too many rapid crashes; giving up on restart.",
                                        );
                                    }
                                }
                                return Ok(code);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_gives_up_after_consecutive_rapid_failures() {
        let mut g = RestartGuard::new();
        // First MAX_RAPID_RESTARTS-1 rapid deaths still allow a restart.
        for _ in 0..(MAX_RAPID_RESTARTS - 1) {
            assert!(g.record(Duration::from_secs(1)));
        }
        // The MAX_RAPID_RESTARTS-th rapid death gives up.
        assert!(!g.record(Duration::from_secs(1)));
    }

    #[test]
    fn guard_resets_after_a_healthy_lifetime() {
        let mut g = RestartGuard::new();
        assert!(g.record(Duration::from_secs(1)));
        assert!(g.record(Duration::from_secs(1)));
        // A long-lived child clears the streak.
        assert!(g.record(Duration::from_secs(MIN_HEALTHY_SECS)));
        // So rapid failures can accumulate from scratch again.
        assert!(g.record(Duration::from_secs(1)));
    }

    #[test]
    fn decision_restarts_only_when_enabled_unblocked_and_healthy() {
        assert_eq!(restart_decision(9, true, false, true), Next::Restart);
        // Restart disabled.
        assert_eq!(restart_decision(9, false, false, true), Next::Stop(137));
        // STOP requested.
        assert_eq!(restart_decision(9, true, true, true), Next::Stop(137));
        // Crash-loop guard tripped.
        assert_eq!(restart_decision(9, true, false, false), Next::Stop(137));
        // Exit code follows the 128 + signal convention.
        assert_eq!(restart_decision(11, false, false, false), Next::Stop(139));
        // Deliberate-termination signals never restart, even when fully eligible.
        assert_eq!(
            restart_decision(libc::SIGINT, true, false, true),
            Next::Stop(130)
        );
        assert_eq!(
            restart_decision(libc::SIGTERM, true, false, true),
            Next::Stop(143)
        );
        // A crash/OOM signal (SIGSEGV) still restarts when eligible.
        assert_eq!(restart_decision(11, true, false, true), Next::Restart);
    }

    #[test]
    fn interpret_distinguishes_exit_from_signal() {
        // Linux wait-status encoding: exit code in bits 8..15, signal in bits 0..6.
        assert_eq!(interpret(3 << 8), Wait::Exited(3));
        assert_eq!(interpret(9), Wait::Signalled(9));
    }
}
