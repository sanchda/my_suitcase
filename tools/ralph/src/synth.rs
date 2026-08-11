//! The handoff synthesizer: distill the working agent's result summary into a
//! small, relevance-filtered carry-forward for the next iteration. The raw
//! summary is the ever-present baseline; this is a best-effort improvement over
//! it. Any failure returns the baseline — there is no separate fallback path.

use crate::config::Config;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const MAX_CARRY_FORWARD_BYTES: usize = 1_200;
/// Coarse wall-clock cap for the one-shot synth call; a hung `claude` is killed.
const SYNTH_TIMEOUT_SECS: u64 = 120;

/// Assemble the synthesizer prompt from this turn's summary, the upcoming leaves,
/// and the previous carry-forward. Kept pure for testing.
pub fn build_synth_prompt(summary: &str, upcoming: &[String], prev: &str) -> String {
    let mut p = String::new();
    p.push_str(
        "You are a note-taker for an autonomous coding loop. Given the last \
         iteration's summary and the next few planned tasks, write the smallest \
         set of carry-forward notes the next worker needs — constraints \
         discovered, half-finished threads, or gotchas RELEVANT TO THE UPCOMING \
         TASKS. Omit anything already obvious from the task text. If nothing is \
         worth carrying, output the single word NONE. Never invent a task id or \
         reorder work. Output at most 6 short bullet lines, no preamble.\n\n",
    );
    p.push_str("## Last iteration summary\n");
    p.push_str(summary.trim());
    p.push_str("\n\n## Next planned tasks\n");
    if upcoming.is_empty() {
        p.push_str("(none — backlog nearly complete)\n");
    } else {
        for leaf in upcoming {
            p.push_str("- ");
            p.push_str(leaf.trim());
            p.push('\n');
        }
    }
    p.push_str("\n## Previous carry-forward (may be stale)\n");
    p.push_str(if prev.trim().is_empty() {
        "(none)"
    } else {
        prev.trim()
    });
    p.push('\n');
    p
}

/// Clamp synthesizer output: treat NONE as empty, trim, and hard-cap bytes.
pub fn bound_output(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("none") {
        return String::new();
    }
    if t.len() <= MAX_CARRY_FORWARD_BYTES {
        return t.to_string();
    }
    let mut end = MAX_CARRY_FORWARD_BYTES;
    while !t.is_char_boundary(end) {
        end -= 1;
    }
    t[..end].to_string()
}

/// Produce the next carry-forward. `run` maps a prompt to the model's raw stdout
/// (None on any failure). The baseline is the raw `summary`; a successful,
/// non-empty synth replaces it; NONE collapses to an empty carry-forward.
pub fn synthesize_with(
    summary: &str,
    upcoming: &[String],
    prev: &str,
    run: impl FnOnce(&str) -> Option<String>,
) -> String {
    let prompt = build_synth_prompt(summary, upcoming, prev);
    match run(&prompt) {
        Some(raw) => bound_output(&raw),
        None => summary.trim().to_string(),
    }
}

/// Real spawn: one-shot `claude -p --model <synth_model>`, prompt on stdin, plain
/// text stdout. See [`run_claude_oneshot`] for the mechanics.
pub fn run_claude(cfg: &Config, prompt: &str) -> Option<String> {
    run_claude_oneshot(&cfg.synth_model, SYNTH_TIMEOUT_SECS, prompt)
}

/// One-shot `claude -p --model <model>`, prompt on stdin, plain text stdout.
/// Shared by the handoff synthesizer, the adversarial judge, and `ralph learn`.
/// Returns None on spawn/exit failure OR if the call outlives `timeout_secs`
/// (watchdog kills the process tree). Stdin is written on a separate thread so
/// a full stdin pipe can't deadlock against a full stdout one.
pub fn run_claude_oneshot(model: &str, timeout_secs: u64, prompt: &str) -> Option<String> {
    let mut cmd = Command::new("claude");
    cmd.args([
        "-p",
        "--model",
        model,
        "--output-format",
        "text",
        "--no-session-persistence",
        "--exclude-dynamic-system-prompt-sections",
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    // Own process group so the watchdog's negative-pid SIGKILL reaps the whole
    // tree (claude + any subprocess) rather than orphaning a hung child.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().ok()?;
    let pid = child.id();

    let mut stdin = child.stdin.take()?;
    let mut stdout = child.stdout.take()?;

    // Feed the prompt concurrently; a child that stops reading can't block us.
    let prompt_bytes = prompt.as_bytes().to_vec();
    let writer = thread::spawn(move || {
        let _ = stdin.write_all(&prompt_bytes);
        // Drop closes stdin so claude sees EOF and produces its output.
    });

    // Watchdog: SIGKILL the group if the call outlives the coarse timeout.
    let done = Arc::new(AtomicBool::new(false));
    let killed = Arc::new(AtomicBool::new(false));
    let (done_w, killed_w) = (done.clone(), killed.clone());
    let watchdog = thread::spawn(move || {
        for _ in 0..timeout_secs * 10 {
            if done_w.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        if !done_w.load(Ordering::SeqCst) {
            killed_w.store(true, Ordering::SeqCst);
            #[cfg(unix)]
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    });

    // Read stdout to EOF on this thread (unblocks on child exit or the kill).
    let mut buf = String::new();
    let read_ok = stdout.read_to_string(&mut buf).is_ok();
    let status = child.wait().ok();
    done.store(true, Ordering::SeqCst);
    let _ = writer.join();
    let _ = watchdog.join();

    if killed.load(Ordering::SeqCst) || !read_ok {
        return None;
    }
    match status {
        Some(s) if s.success() => Some(buf),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_includes_summary_upcoming_and_prev() {
        let p = build_synth_prompt(
            "closed 43.3, DreamMotifState is read-only",
            &["43.4 — audio pass".into()],
            "- previous note",
        );
        assert!(p.contains("closed 43.3"));
        assert!(p.contains("43.4 — audio pass"));
        assert!(p.contains("previous note"));
        assert!(p.contains("NONE"));
    }

    #[test]
    fn bound_output_none_is_empty() {
        assert_eq!(bound_output("  NONE \n"), "");
        assert_eq!(bound_output(""), "");
    }

    #[test]
    fn bound_output_caps_bytes_on_char_boundary() {
        let big = "x".repeat(5_000);
        assert_eq!(bound_output(&big).len(), MAX_CARRY_FORWARD_BYTES);
    }

    #[test]
    fn bound_output_caps_multibyte_on_char_boundary() {
        // 4-byte chars: 1200 is NOT a multiple of 4, so a naive &t[..1200] slice
        // would split a char and panic. Result must be valid UTF-8 and <= cap.
        let big = "🌱".repeat(1_000); // 4000 bytes
        let out = bound_output(&big);
        assert!(out.len() <= MAX_CARRY_FORWARD_BYTES);
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
        // 1200 % 4 == 0 for this glyph, so the walk lands exactly on the cap.
        assert_eq!(out.len(), MAX_CARRY_FORWARD_BYTES);

        // 2-byte char whose boundary straddles the cap (1200 is even, so pad by
        // one ASCII byte to force an odd split point and exercise the backstep).
        let mut odd = String::from("z");
        odd.push_str(&"é".repeat(1_000)); // 1 + 2000 bytes
        let out2 = bound_output(&odd);
        assert!(out2.len() <= MAX_CARRY_FORWARD_BYTES);
        assert!(std::str::from_utf8(out2.as_bytes()).is_ok());
        assert_eq!(out2.len(), MAX_CARRY_FORWARD_BYTES - 1); // stepped back off split
    }

    #[test]
    fn failed_run_falls_back_to_summary() {
        let out = synthesize_with("raw summary", &[], "", |_| None);
        assert_eq!(out, "raw summary");
    }

    #[test]
    fn none_result_yields_empty_carry_forward() {
        let out = synthesize_with("raw summary", &[], "", |_| Some("NONE".into()));
        assert_eq!(out, "");
    }

    #[test]
    fn success_replaces_baseline() {
        let out = synthesize_with("raw summary", &[], "", |_| {
            Some("- keep foo.rs guard".into())
        });
        assert_eq!(out, "- keep foo.rs guard");
    }
}
