//! `ralph stop [--now]` — request a graceful halt, and optionally signal the
//! running loop so a hung turn doesn't outlast the request.

use crate::state::State;
use crate::{config, pidguard, supervisor, R};
use std::path::Path;

const USAGE: &str = "\
Usage: ralph stop [--now] [--dir <path>] [--config <file>]

Writes STOP, honored after the current task. --now also SIGTERMs the running
loop, which tears down its claude session on the way out.
";

/// SIGTERM the recorded loop, if one is alive. `Ok(None)` means nothing to
/// signal — the STOP marker is all the caller gets.
fn signal_loop(dir: &Path) -> R<Option<u32>> {
    let Some(pid) = pidguard::running(&supervisor::pidfile(dir)) else {
        return Ok(None);
    };
    // pid-to-pid, never `kill(-pid, …)`: a hand-launched ralph sits in the
    // invoking shell's process group and would take the shell down with it.
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } != 0 {
        let err = std::io::Error::last_os_error();
        return Err(format!("signalling loop pid {pid}: {err}").into());
    }
    Ok(Some(pid))
}

pub fn run(args: &[String]) -> R<i32> {
    let now = args.iter().any(|a| a == "--now");
    // Strip our own flag before config resolution, which rejects unknown args.
    let rest: Vec<String> = args
        .iter()
        .filter(|a| a.as_str() != "--now")
        .cloned()
        .collect();
    let mut cfg = config::load_base(&rest)?;
    if config::apply_args(&mut cfg, &rest)? {
        print!("{USAGE}");
        return Ok(0);
    }
    config::validate(&cfg)?;

    let state = State::open(&cfg.dir)?;
    state.request_stop()?;
    println!(
        "ralph: stop requested → {} (loop halts after the current task; suppresses --restart)",
        cfg.dir.join("STOP").display()
    );
    if now {
        match signal_loop(&cfg.dir)? {
            Some(pid) => println!("ralph: SIGTERM → pid {pid} (loop and claude stop now)"),
            None => println!(
                "ralph: no live loop recorded in {} — STOP will be honored when one starts",
                supervisor::pidfile(&cfg.dir).display()
            ),
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn tmp() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ralph-stop-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn signalling_without_a_pidfile_is_a_no_op() {
        let dir = tmp();
        assert_eq!(signal_loop(&dir).unwrap(), None);
        // A stale pidfile is cleared rather than signalled.
        std::fs::write(supervisor::pidfile(&dir), "4000000000\n").unwrap();
        assert_eq!(signal_loop(&dir).unwrap(), None);
        assert!(!supervisor::pidfile(&dir).exists());
    }

    #[test]
    fn signal_reaches_the_recorded_pid() {
        let dir = tmp();
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        std::fs::write(supervisor::pidfile(&dir), format!("{}\n", child.id())).unwrap();

        assert_eq!(signal_loop(&dir).unwrap(), Some(child.id()));

        let status = child.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }
}
