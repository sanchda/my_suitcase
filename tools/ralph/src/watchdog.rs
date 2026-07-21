//! Detached death-watchdog: reports ralph's *ungraceful* exit (OOM/kill/crash),
//! which ralph itself can't announce because SIGKILL runs no cleanup.
//!
//! Armed once, early, while ralph is still single-threaded — so the classic
//! double-`fork` daemonization is safe here (no other thread can hold a lock the
//! forked child would inherit locked). The grandchild is reparented to init,
//! outlives ralph, and polls `/proc/<ralph-pid>`. A sentinel file distinguishes a
//! clean exit from a kill: [`Guard`]'s `Drop` removes it on any normal return or
//! panic unwind, but SIGKILL runs no `Drop` — so the watchdog fires only when the
//! sentinel is still present, i.e. ralph died without cleanup.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::notify;

const POLL: Duration = Duration::from_secs(10);
const SENTINEL: &str = "watchdog.alive";

/// Removes the sentinel on drop → the watchdog treats the exit as graceful.
pub struct Guard {
    sentinel: PathBuf,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.sentinel);
    }
}

/// Double-fork a detached watchdog for ralph's death. Call once, early, before
/// any thread is spawned. Returns a guard whose drop stands the watchdog down;
/// `None` when no webhook is configured or the fork fails.
pub fn arm(dir: &Path, webhook: &str) -> Option<Guard> {
    if webhook.trim().is_empty() {
        return None;
    }
    let sentinel = dir.join(SENTINEL);
    if std::fs::write(&sentinel, std::process::id().to_string()).is_err() {
        return None;
    }
    let parent = std::process::id();
    // SAFETY: called before ralph spawns any thread, so the process is
    // single-threaded and fork-without-exec cannot inherit a locked mutex.
    match unsafe { libc::fork() } {
        -1 => {
            let _ = std::fs::remove_file(&sentinel);
            None
        }
        0 => unsafe {
            // Intermediate child: new session, fork the real watchdog, then exit
            // so the grandchild reparents to init and is fully detached.
            libc::setsid();
            if libc::fork() == 0 {
                detach_stdio();
                watch(parent, dir, webhook);
            }
            libc::_exit(0);
        },
        mid => {
            // Parent (ralph): reap the intermediate child, then keep running.
            let mut status = 0;
            unsafe { libc::waitpid(mid, &mut status, 0) };
            Some(Guard { sentinel })
        }
    }
}

/// Grandchild loop: wait for ralph to vanish, then report it unless ralph exited
/// gracefully (sentinel already removed).
fn watch(parent: u32, dir: &Path, webhook: &str) {
    while alive(parent) {
        thread::sleep(POLL);
    }
    let sentinel = dir.join(SENTINEL);
    if !sentinel.exists() {
        return; // graceful exit removed it — ralph announced its own outcome.
    }
    let _ = std::fs::remove_file(&sentinel);
    let tail = std::fs::read_to_string(dir.join("iteration"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|n| format!(", last iter {n}"))
        .unwrap_or_default();
    if let Some(n) = notify::Notifier::new(webhook) {
        n.post(&format!(
            "💀 **ralph terminated** — pid {parent} died with no graceful shutdown \
             (OOM, kill, or crash){tail}. Relaunch to resume."
        ));
    }
}

/// True while `pid` is a live `ralph` process (comm guards against pid reuse).
fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|comm| comm.trim() == "ralph")
        .unwrap_or(false)
}

/// Redirect stdio to /dev/null so the detached watchdog holds no terminal.
unsafe fn detach_stdio() {
    let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
    if devnull >= 0 {
        libc::dup2(devnull, 0);
        libc::dup2(devnull, 1);
        libc::dup2(devnull, 2);
        if devnull > 2 {
            libc::close(devnull);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonexistent_pid_is_not_alive() {
        assert!(!alive(4_000_000_000));
    }

    #[test]
    fn non_ralph_process_is_not_alive() {
        // Our own pid exists, but the test binary's comm is not "ralph".
        assert!(!alive(std::process::id()));
    }

    #[test]
    fn arm_disabled_without_webhook() {
        assert!(arm(Path::new("/tmp"), "").is_none());
    }
}
