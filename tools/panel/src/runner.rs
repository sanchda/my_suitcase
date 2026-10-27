//! One-shot model calls and the bounded-concurrency pool that issues them.
//!
//! Every call is a fresh process with a fresh context. That is the whole cost
//! story: a persistent voter re-sends its accumulated history on every pair, so
//! the token bill grows quadratically in the number of pairs. A fresh voter's
//! context is flat, and it cannot drift as the run proceeds either.
//!
//! The subprocess handling mirrors `ralph`'s `synth::run_oneshot` — own process
//! group so the watchdog's negative-pid SIGKILL reaps the whole tree, and stdin
//! written on its own thread so a full stdin pipe cannot deadlock against a
//! full stdout one.

use serde_json::Value;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub output_tokens: u64,
    pub duration_ms: u64,
}

impl Usage {
    pub fn absorb(&mut self, other: &Usage) {
        self.cost_usd += other.cost_usd;
        self.input_tokens += other.input_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.output_tokens += other.output_tokens;
        self.duration_ms += other.duration_ms;
    }

    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
            + self.output_tokens
    }
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub usage: Usage,
}

fn is_codex(model: &str) -> bool {
    let bare = model.strip_prefix('!').unwrap_or(model);
    let family = bare.split('-').next().unwrap_or(bare);
    bare == "codex" || matches!(family, "gpt" | "chatgpt" | "codex" | "o1" | "o3" | "o4")
}

/// Run one prompt to completion. `Err` on spawn failure, non-zero exit, an
/// error envelope, or a call that outlives `timeout_secs`.
///
/// `cwd` is the directory the voter starts in, and it is load-bearing rather
/// than incidental: a voter is a real agent with a real filesystem, so the
/// cheapest way to stop it finding the document being ranked is to start it
/// somewhere that document is not.
pub fn run_oneshot(
    model: &str,
    timeout_secs: u64,
    prompt: &str,
    cwd: &Path,
) -> Result<Completion, String> {
    let codex = is_codex(model);
    let mut cmd = Command::new(if codex { "codex" } else { "claude" });
    if codex {
        cmd.args(["exec", "--json", "--sandbox", "read-only"]);
        cmd.args(["-c", "approval_policy=\"never\""]);
        cmd.arg("--ephemeral");
        if model != "codex" {
            cmd.args(["--model", model]);
        }
        cmd.arg("-");
    } else {
        cmd.args([
            "-p",
            "--model",
            model,
            "--output-format",
            "json",
            "--no-session-persistence",
            "--exclude-dynamic-system-prompt-sections",
        ]);
    }
    cmd.current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd.spawn().map_err(|e| format!("spawn {model}: {e}"))?;
    let pid = child.id();

    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let mut stderr = child.stderr.take().ok_or("no stderr")?;

    let bytes = prompt.as_bytes().to_vec();
    let writer = thread::spawn(move || {
        let _ = stdin.write_all(&bytes);
    });
    let diagnostic = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

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
        killed_w.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    });

    let mut raw = String::new();
    let read = stdout.read_to_string(&mut raw);
    let status = child.wait();
    done.store(true, Ordering::SeqCst);
    let _ = watchdog.join();
    let _ = writer.join();
    let stderr_text = diagnostic.join().unwrap_or_default();

    if killed.load(Ordering::SeqCst) {
        return Err(format!("{model}: timed out after {timeout_secs}s"));
    }
    read.map_err(|e| format!("{model}: reading stdout: {e}"))?;
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            let tail: String = stderr_text.chars().rev().take(300).collect::<String>()
                .chars().rev().collect();
            return Err(format!("{model}: exited {s}: {}", tail.trim()));
        }
        Err(e) => return Err(format!("{model}: wait: {e}")),
    }

    parse_envelope(&raw, codex).ok_or_else(|| {
        format!(
            "{model}: no usable result in output ({} bytes)",
            raw.len()
        )
    })
}

/// Pull the final result out of the backend's JSON. Claude emits one object
/// with `--output-format json`; codex emits JSONL, so we take the last usable
/// result line. Codex reports no USD cost, so its `cost_usd` stays 0.
pub fn parse_envelope(raw: &str, codex: bool) -> Option<Completion> {
    let mut found: Option<Completion> = None;
    let candidates: Vec<&str> = if codex {
        raw.lines().collect()
    } else {
        vec![raw.trim()]
    };
    for line in candidates {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("is_error").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let text = v
            .get("result")
            .or_else(|| v.get("last_agent_message"))
            .or_else(|| v.pointer("/msg/message"))
            .and_then(Value::as_str);
        let Some(text) = text else { continue };
        let usage_at = |field: &str| {
            v.get("usage")
                .and_then(|u| u.get(field))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        };
        found = Some(Completion {
            text: text.to_string(),
            usage: Usage {
                cost_usd: v.get("total_cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
                input_tokens: usage_at("input_tokens"),
                cache_creation_input_tokens: usage_at("cache_creation_input_tokens"),
                cache_read_input_tokens: usage_at("cache_read_input_tokens"),
                output_tokens: usage_at("output_tokens"),
                duration_ms: v.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
            },
        });
    }
    found
}

/// Run `jobs` at most `width` at a time, preserving input order in the output.
pub fn pool<T, R>(jobs: Vec<T>, width: usize, work: impl Fn(usize, T) -> R + Sync + Send) -> Vec<R>
where
    T: Send,
    R: Send,
{
    let total = jobs.len();
    if total == 0 {
        return Vec::new();
    }
    let width = width.max(1).min(total);
    let slots: Vec<Mutex<Option<R>>> = (0..total).map(|_| Mutex::new(None)).collect();
    let queue: Vec<Mutex<Option<T>>> = jobs.into_iter().map(|j| Mutex::new(Some(j))).collect();
    let cursor = AtomicUsize::new(0);

    thread::scope(|scope| {
        for _ in 0..width {
            scope.spawn(|| loop {
                let next = cursor.fetch_add(1, Ordering::SeqCst);
                if next >= total {
                    return;
                }
                let Some(job) = queue[next].lock().unwrap().take() else {
                    continue;
                };
                let value = work(next, job);
                *slots[next].lock().unwrap() = Some(value);
            });
        }
    });

    slots
        .into_iter()
        .map(|slot| slot.into_inner().unwrap().expect("every slot filled"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_claude_result_envelope_with_usage() {
        let raw = r#"{"type":"result","subtype":"success","is_error":false,
          "result":"1|A|0.8|because","total_cost_usd":0.42,"duration_ms":9100,
          "usage":{"input_tokens":5,"cache_creation_input_tokens":100,
          "cache_read_input_tokens":300,"output_tokens":40}}"#;
        let c = parse_envelope(raw, false).unwrap();
        assert_eq!(c.text, "1|A|0.8|because");
        assert_eq!(c.usage.cost_usd, 0.42);
        assert_eq!(c.usage.output_tokens, 40);
        assert_eq!(c.usage.total_tokens(), 445);
    }

    #[test]
    fn an_error_envelope_is_not_a_result() {
        assert!(parse_envelope(r#"{"type":"result","is_error":true,"result":"limit"}"#, false).is_none());
    }

    #[test]
    fn takes_the_last_usable_line_of_codex_jsonl() {
        let raw = "{\"type\":\"item.started\"}\nnot json\n\
                   {\"type\":\"turn.completed\",\"last_agent_message\":\"1|B|0.5|x\"}\n";
        assert_eq!(parse_envelope(raw, true).unwrap().text, "1|B|0.5|x");
    }

    #[test]
    fn usage_absorbs_across_calls() {
        let mut total = Usage::default();
        for _ in 0..3 {
            total.absorb(&Usage { cost_usd: 0.5, output_tokens: 10, ..Usage::default() });
        }
        assert_eq!(total.cost_usd, 1.5);
        assert_eq!(total.output_tokens, 30);
    }

    #[test]
    fn the_pool_preserves_order_under_concurrency() {
        let out = pool((0..64).collect(), 8, |_, n: i32| {
            thread::sleep(Duration::from_millis((n % 5) as u64));
            n * 2
        });
        assert_eq!(out, (0..64).map(|n| n * 2).collect::<Vec<_>>());
    }

    #[test]
    fn the_pool_handles_an_empty_job_list() {
        let out: Vec<i32> = pool(Vec::<i32>::new(), 4, |_, n| n);
        assert!(out.is_empty());
    }

    #[test]
    fn codex_models_are_recognized() {
        assert!(is_codex("codex"));
        assert!(is_codex("gpt-5"));
        assert!(!is_codex("opus"));
        assert!(!is_codex("claude-sonnet-5"));
    }
}
