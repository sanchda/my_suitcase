//! Consuming `claude`'s `stream-json` (NDJSON) stdout as it arrives.
//!
//! Each line is tee'd verbatim to the raw iteration log (for `tail -f`) and
//! parsed to update a live [`IterStatus`] (current tool, event count, output
//! tokens, last activity). The final `{"type":"result",...}` envelope is
//! captured and returned. Malformed lines are tee'd and skipped — a bad line
//! never crashes the loop.

use serde_json::Value;
use std::io::{BufRead, Write};
use std::time::{SystemTime, UNIX_EPOCH};

/// The parsed final result envelope. The CLI's exit code is unreliable, so this
/// is the source of truth for classifying an iteration.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultEnvelope {
    pub is_error: bool,
    pub api_error_status: Option<u16>,
    pub result: String,
    pub total_cost_usd: f64,
    pub duration_ms: u64,
    pub duration_api_ms: u64,
    pub num_turns: u64,
    pub input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub output_tokens: u64,
    /// The original JSON line, for persisting to `last-result.json`.
    pub raw: String,
}

/// Live, human-readable status of the running iteration.
#[derive(Debug, Clone)]
pub struct IterStatus {
    pub iter: u64,
    pub model: String,
    pub start_secs: u64,
    pub events: u64,
    pub current_tool: Option<String>,
    pub last_text: Option<String>,
    pub out_tokens: u64,
}

impl IterStatus {
    pub fn new(iter: u64, model: &str) -> Self {
        IterStatus {
            iter,
            model: model.to_string(),
            start_secs: now_secs(),
            events: 0,
            current_tool: None,
            last_text: None,
            out_tokens: 0,
        }
    }

    /// Render the multi-line status written to `.ralph/live`.
    pub fn render(&self) -> String {
        let elapsed = now_secs().saturating_sub(self.start_secs);
        let mut s = format!(
            "iter {} | model {} | elapsed {}m{:02}s | events {} | out~{} tok\n",
            self.iter,
            self.model,
            elapsed / 60,
            elapsed % 60,
            self.events,
            self.out_tokens,
        );
        if let Some(t) = &self.current_tool {
            s.push_str(&format!("tool: {t}\n"));
        }
        if let Some(t) = &self.last_text {
            let snip: String = t.chars().take(200).collect();
            s.push_str(&format!("last: {}\n", snip.replace('\n', " ")));
        }
        s
    }
}

/// Consume the NDJSON `reader`, tee-ing raw lines to `raw`, updating `status`,
/// and calling `emit(&status)` after each line so the caller can persist the
/// live view. Returns the final result envelope if one was seen.
pub fn consume<Rd, W, F>(
    reader: Rd,
    raw: &mut W,
    status: &mut IterStatus,
    mut emit: F,
) -> std::io::Result<Option<ResultEnvelope>>
where
    Rd: BufRead,
    W: Write,
    F: FnMut(&IterStatus),
{
    let mut parser = Events::default();
    for line in reader.lines() {
        let line = line?;
        writeln!(raw, "{line}")?;
        raw.flush()?;
        status.events += 1;
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            update_status(status, &v);
            parser.observe(&v, &line);
            if let Some(e) = &parser.envelope {
                status.out_tokens = e.output_tokens;
            }
        }
        emit(status);
    }
    Ok(parser.envelope)
}

/// Adapt Codex events to the existing result envelope. Only the last completed
/// agent message is final text; reasoning and tool output can never complete a run.
#[derive(Default)]
pub struct Events {
    pub envelope: Option<ResultEnvelope>,
    pub thread_id: Option<String>,
    final_text: String,
}

impl Events {
    pub fn observe(&mut self, v: &Value, line: &str) {
        match v["type"].as_str().unwrap_or("") {
            "result" => self.envelope = Some(parse_envelope(v, line)),
            "thread.started" => self.thread_id = v["thread_id"].as_str().map(String::from),
            "turn.started" => {
                self.final_text.clear();
                self.envelope = None;
            }
            "item.completed" if v["item"]["type"] == "agent_message" => {
                self.final_text = v["item"]["text"].as_str().unwrap_or("").to_string();
            }
            "turn.completed" => {
                let usage = &v["usage"];
                let cached = usage["cached_input_tokens"].as_u64().unwrap_or(0);
                let normalized = serde_json::json!({
                    "type": "result", "is_error": false, "result": self.final_text,
                    "num_turns": 1, "total_cost_usd": 0.0,
                    "usage": {
                        "input_tokens": usage["input_tokens"].as_u64().unwrap_or(0).saturating_sub(cached),
                        "cache_read_input_tokens": cached,
                        "output_tokens": usage["output_tokens"].as_u64().unwrap_or(0),
                    }
                });
                self.envelope = Some(parse_envelope(&normalized, &normalized.to_string()));
            }
            // An error event may describe a reconnect attempt. A subsequent
            // completed turn supersedes it; a stream ending here is a failure.
            "turn.failed" | "error" => {
                let err = v.get("error").unwrap_or(v);
                let message = err
                    .as_str()
                    .or_else(|| err["message"].as_str())
                    .unwrap_or(line);
                let status = err
                    .get("status_code")
                    .or_else(|| err.get("http_status_code"));
                let normalized = serde_json::json!({
                    "type": "result", "is_error": true, "result": message,
                    "api_error_status": status,
                });
                self.envelope = Some(parse_envelope(&normalized, &normalized.to_string()));
            }
            _ => {}
        }
    }
}

pub fn error_envelope(text: &str) -> ResultEnvelope {
    let value = serde_json::json!({"type":"result", "is_error":true, "result":text});
    parse_envelope(&value, &value.to_string())
}

/// Fold one parsed event into the live status.
fn update_status(status: &mut IterStatus, v: &Value) {
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
    if ty.starts_with("item.") {
        let item = &v["item"];
        match item["type"].as_str().unwrap_or("") {
            "agent_message" => status.last_text = item["text"].as_str().map(String::from),
            "command_execution" | "file_change" | "mcp_tool_call" | "web_search" => {
                status.current_tool = if ty == "item.completed" {
                    None
                } else {
                    item["command"]
                        .as_str()
                        .or_else(|| item["tool"].as_str())
                        .or_else(|| item["type"].as_str())
                        .map(String::from)
                };
            }
            _ => {}
        }
    }
    if ty == "assistant" {
        if let Some(msg) = v.get("message") {
            if let Some(content) = msg.get("content").and_then(Value::as_array) {
                for block in content {
                    match block.get("type").and_then(Value::as_str) {
                        Some("tool_use") => {
                            if let Some(name) = block.get("name").and_then(Value::as_str) {
                                status.current_tool = Some(name.to_string());
                            }
                        }
                        Some("text") => {
                            if let Some(t) = block.get("text").and_then(Value::as_str) {
                                if !t.trim().is_empty() {
                                    status.last_text = Some(t.to_string());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            if let Some(out) = msg
                .get("usage")
                .and_then(|u| u.get("output_tokens"))
                .and_then(Value::as_u64)
            {
                // Usage is cumulative per assistant turn; keep the max seen.
                status.out_tokens = status.out_tokens.max(out);
            }
        }
    }
}

/// Extract the result envelope fields, coercing `api_error_status` from either a
/// JSON number or string.
fn parse_envelope(v: &Value, raw_line: &str) -> ResultEnvelope {
    let api_error_status = v.get("api_error_status").and_then(|s| match s {
        Value::Number(n) => n.as_u64().map(|x| x as u16),
        Value::String(s) => s.trim().parse::<u16>().ok(),
        _ => None,
    });
    let usage = v.get("usage");
    let usage_u64 = |field: &str| {
        usage
            .and_then(|value| value.get(field))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    ResultEnvelope {
        is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        api_error_status,
        result: v
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        total_cost_usd: v
            .get("total_cost_usd")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        duration_ms: v.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
        duration_api_ms: v
            .get("duration_api_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        num_turns: v.get("num_turns").and_then(Value::as_u64).unwrap_or(0),
        input_tokens: usage_u64("input_tokens"),
        cache_creation_input_tokens: usage_u64("cache_creation_input_tokens"),
        cache_read_input_tokens: usage_u64("cache_read_input_tokens"),
        output_tokens: usage_u64("output_tokens"),
        raw: raw_line.to_string(),
    }
}

/// Whole-line marker match: a line whose trimmed content equals `marker`.
/// Mentioning the marker inside prose must not count (mirrors `grep -qxE`).
pub fn has_marker(text: &str, marker: &str) -> bool {
    text.lines().any(|l| l.trim() == marker)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn codex_uses_only_the_last_agent_message_of_a_completed_turn() {
        let input = concat!(
            "{\"type\":\"turn.started\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"RALPH_COMPLETE\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"still working\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"reasoning\",\"text\":\"RALPH_COMPLETE\"}}\n",
        );
        assert!(
            drain(input).0.is_none(),
            "a partial stream cannot complete a turn"
        );
        let (env, _, raw) = drain(&format!(
            "{input}{{\"type\":\"turn.completed\",\"usage\":{{}}}}\n"
        ));
        assert_eq!(env.unwrap().result, "still working");
        assert!(raw.starts_with(input));
    }

    #[test]
    fn codex_failures_and_recovered_errors() {
        let fail = r#"{"type":"turn.failed","error":{"message":"rate limit","status_code":"429"}}"#;
        let env = drain(fail).0.unwrap();
        assert!(env.is_error);
        assert_eq!(env.api_error_status, Some(429));
        let recovered = format!("{fail}\n{{\"type\":\"turn.completed\",\"usage\":{{}}}}\n");
        assert!(!drain(&recovered).0.unwrap().is_error);
    }

    fn drain(input: &str) -> (Option<ResultEnvelope>, IterStatus, String) {
        let mut raw: Vec<u8> = Vec::new();
        let mut status = IterStatus::new(1, "sonnet");
        let env = consume(Cursor::new(input), &mut raw, &mut status, |_| {}).unwrap();
        (env, status, String::from_utf8(raw).unwrap())
    }

    #[test]
    fn tees_all_lines_verbatim() {
        let input = "{\"type\":\"system\"}\nnot json\n{\"type\":\"result\",\"is_error\":false,\"result\":\"ok\"}\n";
        let (_, _, raw) = drain(input);
        assert_eq!(raw, input);
    }

    #[test]
    fn captures_success_envelope() {
        let input = r#"{"type":"result","subtype":"success","is_error":false,"result":"done here","total_cost_usd":0.42,"duration_ms":9100,"duration_api_ms":7200,"num_turns":12,"usage":{"input_tokens":5,"cache_creation_input_tokens":100,"cache_read_input_tokens":300,"output_tokens":40}}"#;
        let (env, _, _) = drain(input);
        let env = env.expect("envelope");
        assert!(!env.is_error);
        assert_eq!(env.result, "done here");
        assert_eq!(env.total_cost_usd, 0.42);
        assert_eq!(env.api_error_status, None);
        assert_eq!(env.duration_ms, 9_100);
        assert_eq!(env.duration_api_ms, 7_200);
        assert_eq!(env.num_turns, 12);
        assert_eq!(env.input_tokens, 5);
        assert_eq!(env.cache_creation_input_tokens, 100);
        assert_eq!(env.cache_read_input_tokens, 300);
        assert_eq!(env.output_tokens, 40);
    }

    #[test]
    fn api_error_status_number_or_string() {
        let (env, _, _) = drain(r#"{"type":"result","is_error":true,"api_error_status":429}"#);
        assert_eq!(env.unwrap().api_error_status, Some(429));
        let (env, _, _) = drain(r#"{"type":"result","is_error":true,"api_error_status":"503"}"#);
        assert_eq!(env.unwrap().api_error_status, Some(503));
    }

    #[test]
    fn malformed_lines_are_skipped_not_fatal() {
        let input = "garbage\n{oops\n{\"type\":\"result\",\"is_error\":false,\"result\":\"x\"}\n";
        let (env, status, _) = drain(input);
        assert!(env.is_some());
        assert_eq!(status.events, 3);
    }

    #[test]
    fn tracks_current_tool_and_tokens() {
        let input = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}],"usage":{"output_tokens":120}}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"working"}],"usage":{"output_tokens":90}}}"#,
            "\n"
        );
        let (_, status, _) = drain(input);
        assert_eq!(status.current_tool.as_deref(), Some("Bash"));
        assert_eq!(status.last_text.as_deref(), Some("working"));
        assert_eq!(status.out_tokens, 120); // max seen
    }

    #[test]
    fn no_envelope_returns_none() {
        let (env, _, _) = drain("{\"type\":\"system\"}\n{\"type\":\"assistant\",\"message\":{}}\n");
        assert!(env.is_none());
    }

    #[test]
    fn marker_whole_line_only() {
        assert!(has_marker("all done\nRALPH_COMPLETE\n", "RALPH_COMPLETE"));
        assert!(has_marker("  RALPH_COMPLETE  ", "RALPH_COMPLETE"));
        assert!(!has_marker(
            "do not emit RALPH_COMPLETE inline",
            "RALPH_COMPLETE"
        ));
    }
}
