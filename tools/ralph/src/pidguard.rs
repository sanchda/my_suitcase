//! Exclusive ownership of a pidfile. Publish a fully written file with an
//! atomic hard link (or a symlink where hard links are unavailable). A short
//! atomic directory claim serializes publication and stale-file reclamation on
//! drvfs/9p where advisory locks can be unreliable.

use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::{symlink, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Ownership of a pidfile; the file is removed when this drops. A process killed
/// by a signal leaves it behind, which is what the liveness probe is for.
pub struct Guard {
    path: PathBuf,
    identity: (u64, u64),
    backing: Option<PathBuf>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        // A stale reclaimer or another process may have replaced the pathname.
        // Never remove a successor's guard from this destructor.
        if identity(&self.path) == Some(self.identity) {
            clear(&self.path);
        }
        if let Some(backing) = &self.backing {
            let _ = fs::remove_file(backing);
        }
    }
}

fn identity(path: &Path) -> Option<(u64, u64)> {
    fs::symlink_metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

/// Serialize all pidfile publication and reclamation. A process killed in this
/// short window leaves the directory behind and blocks future acquisition.
struct ReclaimGuard(PathBuf);

impl Drop for ReclaimGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

fn reclaim_path(path: &Path) -> PathBuf {
    let name = format!(
        "{}.reap",
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    path.with_file_name(name)
}

fn reclaim_guard(path: &Path) -> io::Result<ReclaimGuard> {
    let claim = reclaim_path(path);
    fs::create_dir(&claim)?;
    Ok(ReclaimGuard(claim))
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
    // A terminated child may remain a zombie until its parent reaps it.
    // It can no longer do work and must not hold a stop waiter indefinitely.
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        if stat
            .rsplit_once(") ")
            .is_some_and(|(_, tail)| tail.starts_with('Z') || tail.starts_with('X'))
        {
            return false;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdshortinfo>() as libc::c_int;
        let got = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDT_SHORTBSDINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        // macOS returns zero bytes for zombies, even though kill(pid, 0)
        // continues to succeed until their parent reaps them.
        if got == 0 || (got == size && info.pbsi_status == libc::SZOMB) {
            return false;
        }
    }
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Read the recorded pid, if any and parseable.
pub fn read(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// The holder's pid, if one is recorded and still alive. Only acquisition
/// reclaims stale files.
pub fn running(path: &Path) -> Option<u32> {
    // Probes never mutate the guard: a new owner could publish between a read
    // of an absent/stale file and its removal.
    read(path).filter(|pid| is_alive(*pid))
}

/// Drop the pidfile (best-effort), e.g. after a confirmed stop.
pub fn clear(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// An empty regular file may have been made by an older Ralph process that has
/// not written its PID yet. Give that process a short window to finish.
fn initializing(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|s| s.trim().is_empty())
        && fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age.as_secs() < 2)
}

/// Publish a complete PID atomically. The temporary file is never read as the
/// guard; the hard link or symlink appears at `path` in one filesystem operation.
fn create(path: &Path) -> io::Result<Guard> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!(
        ".{}.{}.{stamp}.{}.owner",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let backing = path.with_file_name(name);
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backing)?;
    let published = (|| {
        f.write_all(format!("{}\n", std::process::id()).as_bytes())?;
        f.sync_all()?;
        match fs::hard_link(&backing, path) {
            Ok(()) => Ok(false),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(e),
            Err(_) => {
                symlink(backing.file_name().unwrap(), path)?;
                Ok(true)
            }
        }
    })();
    match published {
        Ok(keep_backing) => {
            let id = identity(path).ok_or_else(|| io::Error::from(ErrorKind::NotFound))?;
            if !keep_backing {
                let _ = fs::remove_file(&backing);
            }
            Ok(Guard {
                path: path.to_path_buf(),
                identity: id,
                backing: keep_backing.then_some(backing),
            })
        }
        Err(e) => {
            let _ = fs::remove_file(backing);
            Err(e)
        }
    }
}

/// Take `path` for this process. `Err(Some(pid))` is a live holder (the caller
/// reports "already running"); `Err(None)` is an I/O failure or a busy claim.
pub fn acquire(path: &Path) -> Result<Guard, Option<u32>> {
    if let Some(pid) = running(path) {
        return Err(Some(pid));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| None)?;
    }
    // Fresh creators must take the claim too: otherwise one can publish into
    // the gap between a reclaimer's liveness check and its unlink.
    let _claim = reclaim_guard(path).map_err(|_| running(path))?;
    match create(path) {
        Ok(guard) => return Ok(guard),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(_) => return Err(None),
    }
    if let Some(pid) = read(path).filter(|pid| is_alive(*pid)) {
        return Err(Some(pid));
    }
    if initializing(path) {
        return Err(None);
    }
    clear(path);
    match create(path) {
        Ok(guard) => Ok(guard),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(read(path)),
        Err(_) => Err(None),
    }
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
    fn unreaped_zombie_does_not_count_as_a_live_guard_holder() {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(!is_alive(child.id()));
        child.wait().unwrap();
    }

    #[test]
    fn running_returns_live_pid_and_leaves_stale_guard_for_acquire() {
        let path = tmp();
        write_pid(&path, std::process::id());
        assert_eq!(running(&path), Some(std::process::id()));
        write_pid(&path, 4_000_000_000);
        assert_eq!(running(&path), None);
        assert_eq!(read(&path), Some(4_000_000_000));
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

    #[test]
    fn empty_legacy_file_is_not_reaped_while_its_owner_may_be_writing() {
        let path = tmp();
        fs::write(&path, "").unwrap();
        assert!(matches!(acquire(&path), Err(None)));
        assert!(path.exists());
    }

    #[test]
    fn a_busy_reclaimer_cannot_remove_a_stale_guard() {
        let path = tmp();
        write_pid(&path, 4_000_000_000);
        let held = reclaim_guard(&path).unwrap();
        assert!(matches!(acquire(&path), Err(None)));
        assert_eq!(read(&path), Some(4_000_000_000));
        drop(held);
        assert!(acquire(&path).is_ok());
    }

    #[test]
    fn a_busy_reclaimer_blocks_fresh_publication() {
        let path = tmp();
        let held = reclaim_guard(&path).unwrap();
        assert!(matches!(acquire(&path), Err(None)));
        assert!(!path.exists());
        assert_eq!(running(&path), None);
        drop(held);
        let _owner = acquire(&path).unwrap();
        assert_eq!(running(&path), Some(std::process::id()));
    }

    #[test]
    fn simultaneous_acquirers_have_one_holder() {
        use std::sync::{Arc, Barrier};
        let path = tmp();
        let start = Arc::new(Barrier::new(16));
        let winners = Arc::new(AtomicU64::new(0));
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let path = path.clone();
                let start = start.clone();
                let winners = winners.clone();
                std::thread::spawn(move || {
                    start.wait();
                    if let Ok(_guard) = acquire(&path) {
                        winners.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(winners.load(Ordering::SeqCst), 1);
    }
}
