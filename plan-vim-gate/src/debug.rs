//! Optional debug logging, enabled by `PLAN_GATE_DEBUG`. Writes to a file so it
//! never pollutes the decision emitted on stdout (which Claude Code parses).

use std::io::Write;

/// Append a line to the debug log when `PLAN_GATE_DEBUG` is set. Log path is
/// `PLAN_GATE_DEBUG_LOG` or `/tmp/plan-vim-gate.log`. Errors are ignored.
pub fn log(msg: &str) {
    if std::env::var_os("PLAN_GATE_DEBUG").is_none() {
        return;
    }
    let path = std::env::var("PLAN_GATE_DEBUG_LOG")
        .unwrap_or_else(|_| "/tmp/plan-vim-gate.log".to_string());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{msg}");
    }
}
