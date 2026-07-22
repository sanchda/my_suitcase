//! Single-loop enforcement via `<state_dir>/loop.pid`. ralphd writes it when it
//! spawns a loop and consults it (with a liveness probe) to refuse a second
//! `/start` and to re-adopt a still-running loop after a ralphd restart.

use std::path::{Path, PathBuf};

fn pidfile(state_dir: &Path) -> PathBuf {
    state_dir.join("loop.pid")
}

/// Is a process with this pid alive? `kill(pid, 0)` performs the permission/
/// existence check without sending a signal.
pub fn is_alive(pid: u32) -> bool {
    // Only probe a real, single process: 0 targets our own process group and
    // values that don't fit pid_t would wrap negative (a group target), so a
    // corrupted pidfile can't be mistaken for a live loop.
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // Signal 0: no-op delivery, but errors with ESRCH if the pid is gone.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Record the running loop's pid.
pub fn write(state_dir: &Path, pid: u32) -> std::io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(pidfile(state_dir), format!("{pid}\n"))
}

/// Read the recorded pid, if any and parseable.
pub fn read(state_dir: &Path) -> Option<u32> {
    std::fs::read_to_string(pidfile(state_dir))
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// The pid of a live loop, if one is recorded and still alive. A recorded but
/// dead pid is stale — it is removed and `None` returned.
pub fn running(state_dir: &Path) -> Option<u32> {
    match read(state_dir) {
        Some(pid) if is_alive(pid) => Some(pid),
        Some(_) => {
            let _ = std::fs::remove_file(pidfile(state_dir));
            None
        }
        None => None,
    }
}

/// Clear the pidfile (best-effort), e.g. after a confirmed stop.
pub fn clear(state_dir: &Path) {
    let _ = std::fs::remove_file(pidfile(state_dir));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "ralphd-pid-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn current_process_is_alive_bogus_pid_is_not() {
        assert!(is_alive(std::process::id()));
        // Very high pid unlikely to exist.
        assert!(!is_alive(4_000_000_000));
    }

    #[test]
    fn is_alive_rejects_zero_and_oversized_pids() {
        assert!(!is_alive(0));
        assert!(!is_alive(u32::MAX));
    }

    #[test]
    fn running_returns_live_pid_and_clears_stale() {
        let dir = tmp();
        // A live pid (our own) is reported as running.
        write(&dir, std::process::id()).unwrap();
        assert_eq!(running(&dir), Some(std::process::id()));
        // A dead pid is treated as stale and removed.
        write(&dir, 4_000_000_000).unwrap();
        assert_eq!(running(&dir), None);
        assert_eq!(read(&dir), None);
    }
}
