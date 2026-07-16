//! plan-vim-gate — a terminal-native `ExitPlanMode` gate for Claude Code.
//!
//! Wired as a `PermissionRequest` hook matching `ExitPlanMode`. Reads the hook
//! event from stdin, opens the plan in nvim inside a tmux split, blocks until
//! the editor closes, then emits an allow/deny decision on stdout based on the
//! edited buffer. See `docs/superpowers/specs/` for the full design.

mod buffer;
mod decision;
mod editor;
mod hook_io;

/// Shared fallible-result alias used across modules.
pub type R<T> = Result<T, Box<dyn std::error::Error>>;

use decision::{ApproveMode, Outcome};

fn main() {
    if let Err(e) = run() {
        // A hook error (non-zero exit, no valid decision JSON) makes Claude
        // fall back to its own ExitPlanMode dialog — a safe default.
        eprintln!("plan-vim-gate: {e}");
        std::process::exit(1);
    }
}

fn run() -> R<()> {
    let event = hook_io::read_event()?;
    if event.plan.trim().is_empty() {
        return Err("no plan content in hook event (.tool_input.plan)".into());
    }

    // Write the plan (with a directive header) to a scratch file and open it.
    // The NamedTempFile is kept alive until the end of this scope so it isn't
    // deleted out from under the editor.
    let scratch = buffer::write_scratch(&event.plan)?;
    editor::open_and_wait(scratch.path())?;

    let content = std::fs::read_to_string(scratch.path())?;
    let parsed = buffer::parse(&content);

    let outcome = match parsed.decision {
        buffer::Decision::Approve if !parsed.body.trim().is_empty() => Outcome::Approve {
            plan: parsed.body,
            tool_input: event.tool_input,
        },
        // Fail-safe: an "approve" with an empty buffer is treated as a reject.
        buffer::Decision::Approve => Outcome::Reject {
            reason: "The plan buffer was approved but empty — nothing to implement. Please re-plan."
                .to_string(),
        },
        buffer::Decision::Reject => Outcome::Reject {
            reason: parsed.body,
        },
    };

    decision::emit(outcome, ApproveMode::from_env());
    Ok(())
}
