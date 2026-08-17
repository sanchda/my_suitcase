//! Message rendering and the webhook POST.
//!
//! The POST shells out to `curl --config -`: no HTTP crate, no TLS stack to keep
//! patched, and the webhook token travels on stdin instead of argv, so it never
//! shows up in `ps`.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Discord's cap on webhook `content`.
const CONTENT_LIMIT: usize = 2000;
const TRUNCATION_MARKER: &str = " [truncated]";

/// Attempts for a retryable failure, including the first try.
const MAX_ATTEMPTS: u32 = 3;
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub struct Post {
    pub webhook: String,
    pub content: String,
    pub username: Option<String>,
    /// Ask Discord to return the created message (`?wait=true`).
    pub wait: bool,
    pub timeout: u64,
}

/// `[id] message`, clamped to Discord's content limit. The id is fenced in
/// backticks, and backticks and control characters are stripped from it so it
/// cannot break out of the fence.
pub fn render(id: &str, message: &str) -> String {
    let id: String = id
        .chars()
        .filter(|c| *c != '`' && !c.is_control())
        .collect();
    let id = id.trim();
    let message = message.trim_end();
    let joined = if id.is_empty() {
        message.to_string()
    } else {
        format!("`[{id}]` {message}")
    };
    clamp(&joined)
}

/// Truncate on a character boundary, marking that we did.
fn clamp(s: &str) -> String {
    if s.chars().count() <= CONTENT_LIMIT {
        return s.to_string();
    }
    let keep = CONTENT_LIMIT - TRUNCATION_MARKER.chars().count();
    let mut out: String = s.chars().take(keep).collect();
    out.push_str(TRUNCATION_MARKER);
    out
}

pub fn payload(post: &Post) -> String {
    // parse: [] means no mention in the content resolves -- a piped log line or
    // a transcript excerpt containing @everyone must not ping the channel.
    let mut body = serde_json::json!({
        "content": post.content,
        "allowed_mentions": { "parse": [] },
    });
    if let Some(name) = post.username.as_deref().map(str::trim) {
        if !name.is_empty() {
            body["username"] = serde_json::Value::String(name.to_string());
        }
    }
    body.to_string()
}

pub fn url(post: &Post) -> String {
    if !post.wait {
        return post.webhook.clone();
    }
    let sep = if post.webhook.contains('?') { '&' } else { '?' };
    format!("{}{sep}wait=true", post.webhook)
}

/// Drop all but the first few characters of the token (the last path segment),
/// for URLs that have to be printed.
pub fn redact(webhook: &str) -> String {
    let (head, tail) = match webhook.rsplit_once('/') {
        Some(parts) => parts,
        None => return "***".to_string(),
    };
    let shown: String = tail.chars().take(4).collect();
    if shown.is_empty() {
        format!("{head}/***")
    } else {
        format!("{head}/{shown}***")
    }
}

/// One `key = "value"` line with curl's config-file escaping applied.
fn line(key: &str, value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    for c in value.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(c),
        }
    }
    format!("{key} = \"{escaped}\"\n")
}

pub fn curl_config(post: &Post) -> String {
    let mut cfg = String::from("silent\nshow-error\n");
    cfg.push_str(&line("request", "POST"));
    cfg.push_str(&line("url", &url(post)));
    cfg.push_str(&line("header", "Content-Type: application/json"));
    cfg.push_str(&line("max-time", &post.timeout.to_string()));
    // Response body, then the status code alone on the last line.
    cfg.push_str(&line("write-out", "\\n%{http_code}"));
    cfg.push_str(&line("data-binary", &payload(post)));
    cfg
}

pub fn split_response(out: &str) -> (String, u16) {
    match out.trim_end_matches('\n').rsplit_once('\n') {
        Some((body, code)) => (body.trim().to_string(), code.trim().parse().unwrap_or(0)),
        // No body: the whole thing is the status code.
        None => (String::new(), out.trim().parse().unwrap_or(0)),
    }
}

fn retry_after(body: &str) -> Option<Duration> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let secs = value.get("retry_after")?.as_f64()?;
    if secs.is_finite() && secs > 0.0 {
        Some(Duration::from_secs_f64(secs).min(MAX_BACKOFF))
    } else {
        None
    }
}

fn backoff(attempt: u32, body: &str) -> Duration {
    retry_after(body).unwrap_or_else(|| Duration::from_millis(500 * 2u64.pow(attempt - 1)))
}

/// Rate limits and server faults are worth another try; a 4xx means we sent
/// something wrong.
fn retryable(code: u16) -> bool {
    code == 429 || (500..600).contains(&code)
}

/// POST the message. Returns the created message id when `wait` was set and
/// Discord reported one.
pub fn send(post: &Post) -> Result<Option<String>, String> {
    let cfg = curl_config(post);
    let mut last = String::new();

    for attempt in 1..=MAX_ATTEMPTS {
        let (body, code) = curl(&cfg)?;
        if (200..300).contains(&code) {
            return Ok(message_id(&body));
        }
        last = if body.is_empty() {
            format!("HTTP {code}")
        } else {
            format!(
                "HTTP {code}: {}",
                body.chars().take(400).collect::<String>()
            )
        };
        if !retryable(code) || attempt == MAX_ATTEMPTS {
            break;
        }
        std::thread::sleep(backoff(attempt, &body));
    }

    Err(format!("webhook rejected the message ({last})"))
}

fn curl(cfg: &str) -> Result<(String, u16), String> {
    let mut child = Command::new("curl")
        .args(["--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run curl: {e} (is curl on PATH?)"))?;

    child
        .stdin
        .take()
        .ok_or("curl stdin unavailable")?
        .write_all(cfg.as_bytes())
        .map_err(|e| format!("cannot write to curl: {e}"))?;

    let out = child
        .wait_with_output()
        .map_err(|e| format!("curl failed: {e}"))?;

    // A transport failure (DNS, TLS, timeout) never reached Discord.
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!("curl exited with {}", out.status)
        } else {
            err
        });
    }
    Ok(split_response(&String::from_utf8_lossy(&out.stdout)))
}

/// The `id` of the message Discord echoes back with `?wait=true`.
fn message_id(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    Some(value.get("id")?.as_str()?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post_of(content: &str) -> Post {
        Post {
            webhook: "https://discord.com/api/webhooks/123/tok".into(),
            content: content.into(),
            username: None,
            wait: false,
            timeout: 10,
        }
    }

    #[test]
    fn renders_id_prefix() {
        assert_eq!(render("build-42", "done"), "`[build-42]` done");
    }

    #[test]
    fn id_cannot_escape_its_fence() {
        assert_eq!(render("a`b\nc", "hi"), "`[abc]` hi");
    }

    #[test]
    fn empty_id_is_dropped() {
        assert_eq!(render("  ", "hi"), "hi");
    }

    #[test]
    fn long_messages_are_clamped() {
        let out = render("id", &"x".repeat(5000));
        assert_eq!(out.chars().count(), CONTENT_LIMIT);
        assert!(out.ends_with(TRUNCATION_MARKER));
        assert!(out.starts_with("`[id]` x"));
    }

    #[test]
    fn multibyte_clamp_stays_on_char_boundaries() {
        let out = render("", &"\u{1f415}".repeat(3000));
        assert_eq!(out.chars().count(), CONTENT_LIMIT);
    }

    #[test]
    fn payload_omits_blank_username() {
        let mut post = post_of("hi");
        assert!(!payload(&post).contains("username"));
        post.username = Some("  ".into());
        assert!(!payload(&post).contains("username"));
        post.username = Some("bark".into());
        assert!(payload(&post).contains(r#""username":"bark""#));
    }

    #[test]
    fn payload_never_resolves_mentions() {
        let post = post_of("@everyone deploy done");
        assert!(
            payload(&post).contains(r#""allowed_mentions":{"parse":[]}"#),
            "{}",
            payload(&post)
        );
    }

    #[test]
    fn wait_appends_to_the_query() {
        let mut post = post_of("hi");
        assert!(!url(&post).contains("wait"));
        post.wait = true;
        assert!(url(&post).ends_with("?wait=true"));
        post.webhook.push_str("?thread_id=9");
        assert!(url(&post).ends_with("?thread_id=9&wait=true"));
    }

    #[test]
    fn redaction_keeps_the_url_recognizable() {
        assert_eq!(
            redact("https://discord.com/api/webhooks/123/abcdefghij"),
            "https://discord.com/api/webhooks/123/abcd***"
        );
        assert_eq!(redact("nope"), "***");
        assert_eq!(redact("https://x/"), "https://x/***");
    }

    #[test]
    fn curl_config_escapes_and_stays_one_line_per_key() {
        let post = Post {
            content: "say \"hi\"\nagain\\".into(),
            ..post_of("")
        };
        let cfg = curl_config(&post);
        assert!(cfg.contains("url = \"https://discord.com/api/webhooks/123/tok\""));
        assert!(cfg.contains("request = \"POST\""));
        assert!(cfg.contains("data-binary = \""), "{cfg}");
        assert!(!cfg.contains("say \"hi\""), "quotes must be escaped: {cfg}");
        for l in cfg.lines() {
            assert!(!l.is_empty());
        }
    }

    #[test]
    fn response_split() {
        assert_eq!(
            split_response("{\"id\":\"7\"}\n204\n"),
            ("{\"id\":\"7\"}".into(), 204)
        );
        assert_eq!(split_response("\n429"), (String::new(), 429));
        assert_eq!(split_response("204"), (String::new(), 204));
        assert_eq!(split_response("garbage"), (String::new(), 0));
    }

    #[test]
    fn retry_after_is_honored_and_capped() {
        assert_eq!(
            retry_after("{\"retry_after\": 1.5}"),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(retry_after("{\"retry_after\": 9999}"), Some(MAX_BACKOFF));
        assert_eq!(retry_after("{}"), None);
        assert_eq!(retry_after("not json"), None);
        assert_eq!(retry_after("{\"retry_after\": -1}"), None);
    }

    #[test]
    fn only_transient_failures_retry() {
        assert!(retryable(429));
        assert!(retryable(500));
        assert!(retryable(503));
        assert!(!retryable(401));
        assert!(!retryable(404));
        assert!(!retryable(204));
    }

    #[test]
    fn backoff_falls_back_to_exponential() {
        assert_eq!(backoff(1, "{}"), Duration::from_millis(500));
        assert_eq!(backoff(2, "{}"), Duration::from_millis(1000));
    }

    #[test]
    fn message_id_from_wait_response() {
        assert_eq!(
            message_id("{\"id\":\"1234\",\"content\":\"hi\"}").as_deref(),
            Some("1234")
        );
        assert_eq!(message_id("{}"), None);
        assert_eq!(message_id(""), None);
    }
}
