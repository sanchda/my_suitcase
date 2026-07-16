//! Opening the plan in nvim inside a tmux split and blocking until it closes.

use crate::R;
use std::path::Path;
use std::process::Command;

/// Open `path` in nvim in a new tmux split and block until the editor exits.
///
/// Requires running inside tmux (`$TMUX` set) — the hook's own stdio are pipes
/// with no controlling TTY, so an interactive editor can only run in a real
/// pane. When not in tmux this returns an error and the caller lets Claude fall
/// back to its built-in dialog.
///
/// Blocking is race-free via a tmux lock channel: we acquire the lock, spawn a
/// detached pane that releases it when the editor exits, then re-acquire —
/// which blocks until the release. The channel is namespaced by PID so
/// overlapping gates never collide.
pub fn open_and_wait(path: &Path) -> R<()> {
    if std::env::var_os("TMUX").is_none() {
        return Err("not running inside tmux ($TMUX unset); cannot open an editor pane".into());
    }

    let chan = format!("plan-gate-{}", std::process::id());
    // Editor command is configurable (default nvim) so it can be swapped or
    // scripted for tests.
    let editor = std::env::var("PLAN_GATE_EDITOR").unwrap_or_else(|_| "nvim".to_string());

    // Acquire the lock (returns immediately when the channel is free).
    tmux(&["wait-for", "-L", &chan])?;

    // Spawn the editor in a split. `;` (not `&&`) guarantees the lock is
    // released even if the editor exits non-zero, so we never hang.
    let inner = format!(
        "{} {}; tmux wait-for -U {}",
        editor,
        shell_quote(&path.to_string_lossy()),
        chan
    );
    if let Err(e) = tmux(&["split-window", "-v", "sh", "-c", &inner]) {
        // Release the lock we just took before bailing.
        let _ = tmux(&["wait-for", "-U", &chan]);
        return Err(e);
    }

    // Blocks until the pane's `-U` releases the lock (editor closed).
    tmux(&["wait-for", "-L", &chan])?;
    // Final release to leave the channel clean.
    let _ = tmux(&["wait-for", "-U", &chan]);
    Ok(())
}

/// Run a tmux command, erroring on non-zero exit.
fn tmux(args: &[&str]) -> R<()> {
    let status = Command::new("tmux").args(args).status()?;
    if !status.success() {
        return Err(format!("`tmux {}` failed ({status})", args.join(" ")).into());
    }
    Ok(())
}

/// Single-quote a string for `sh -c`, escaping embedded single quotes.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn quotes_plain_path() {
        assert_eq!(shell_quote("/tmp/plan-gate-abc.md"), "'/tmp/plan-gate-abc.md'");
    }

    #[test]
    fn escapes_single_quote() {
        assert_eq!(shell_quote("/tmp/a'b.md"), "'/tmp/a'\\''b.md'");
    }
}
