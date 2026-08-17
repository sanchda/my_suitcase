//! Exclusive ownership of a pidfile. `create_new` is atomic in the kernel, so
//! racing processes need no lock primitive — which matters because the repo can
//! live on drvfs/9p, where advisory locks are unreliable.

use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

/// Ownership of a pidfile; the file is removed when this drops. A process killed
/// by a signal leaves it behind, which is what the liveness probe is for.
pub struct Guard {
    path: PathBuf,
}

impl Drop for Guard {
    fn drop(&mut self) {
        clear(&self.path);
    }
}

/// Is a process with this pid alive? `kill(pid, 0)` performs the permission/
/// existence check without sending a signal.
pub fn is_alive(pid: u32) -> bool {
    // Only probe a real, single process: 0 targets our own process group and
    // values that don't fit pid_t would wrap negative (a group target), so a
    // corrupted pidfile can't be mistaken for a live holder.
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Read the recorded pid, if any and parseable.
pub fn read(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// The holder's pid, if one is recorded and still alive. A recorded but dead pid
/// is stale — it is removed and `None` returned.
pub fn running(path: &Path) -> Option<u32> {
    match read(path) {
        Some(pid) if is_alive(pid) => Some(pid),
        Some(_) => {
            clear(path);
            None
        }
        None => None,
    }
}

/// Drop the pidfile (best-effort), e.g. after a confirmed stop.
pub fn clear(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Create the file exclusively and record our pid.
fn create(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(format!("{}\n", std::process::id()).as_bytes())
        .inspect_err(|_| clear(path))
}

/// Take `path` for this process. `Err(Some(pid))` is a live holder (the caller
/// reports "already running"); `Err(None)` is an I/O failure or a racing creator
/// that won the one retry.
pub fn acquire(path: &Path) -> Result<Guard, Option<u32>> {
    // Two passes: the second is the retry after clearing a stale file.
    for _ in 0..2 {
        match create(path) {
            Ok(()) => {
                return Ok(Guard {
                    path: path.to_path_buf(),
                })
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => match read(path) {
                Some(pid) if is_alive(pid) => return Err(Some(pid)),
                // Dead pid, or garbage we can't probe: the file is stale.
                _ => clear(path),
            },
            Err(_) => return Err(None),
        }
    }
    Err(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "ralph-pidguard-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base.join("loop.pid")
    }

    fn write_pid(path: &Path, pid: u32) {
        std::fs::write(path, format!("{pid}\n")).unwrap();
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
        let path = tmp();
        write_pid(&path, std::process::id());
        assert_eq!(running(&path), Some(std::process::id()));
        write_pid(&path, 4_000_000_000);
        assert_eq!(running(&path), None);
        assert_eq!(read(&path), None);
    }

    #[test]
    fn acquire_records_our_pid_and_releases_on_drop() {
        let path = tmp();
        let guard = acquire(&path).unwrap();
        assert_eq!(read(&path), Some(std::process::id()));
        drop(guard);
        assert!(!path.exists());
    }

    #[test]
    fn acquire_reports_the_live_holder() {
        let path = tmp();
        let _guard = acquire(&path).unwrap();
        assert_eq!(acquire(&path).err(), Some(Some(std::process::id())));
    }

    #[test]
    fn acquire_reclaims_a_stale_or_corrupt_file() {
        let path = tmp();
        write_pid(&path, 4_000_000_000);
        let guard = acquire(&path).unwrap();
        assert_eq!(read(&path), Some(std::process::id()));
        drop(guard);
        // Unparseable content is stale too — it can't name a live process.
        std::fs::write(&path, "not a pid").unwrap();
        assert!(acquire(&path).is_ok());
    }
}
