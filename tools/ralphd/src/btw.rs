//! Driving a streaming `/btw` claude session.
//!
//! `claude -p --output-format stream-json` emits one NDJSON event per line. We
//! fold those into live token accounting (a technique inspired by `cctop`, which
//! surfaces a running session's token usage) and edit a *single* Discord status
//! message every few minutes with elapsed time and tokens-so-far. When the
//! session outlives Discord's 15-minute interaction-token window we delete the
//! deferred reply and continue in a plain channel message (which never expires).
//! On completion the live message becomes the result plus the session's final
//! token count and cost, taken from the authoritative `{"type":"result"}`
//! envelope.

use serenity::all::{
    CommandInteraction, Context, CreateMessage, EditInteractionResponse, EditMessage, MessageId,
};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;

/// Cadence of the progress edits.
const PROGRESS_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Move off the interaction token before Discord's 15-minute expiry (with
/// margin), after which the deferred reply can no longer be edited or deleted.
const TRANSITION_DEADLINE: Duration = Duration::from_secs(14 * 60);
/// Discord's message-content cap.
const DISCORD_LIMIT: usize = 2000;

/// Live, cumulative accounting folded from the event stream.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Stats {
    /// Assistant events seen so far (a rough live activity counter — distinct
    /// from the envelope's authoritative `num_turns`).
    pub steps: u64,
    pub output_tokens: u64,
    pub current_tool: Option<String>,
}

/// The fields we surface from the final `{"type":"result"}` envelope — the
/// authoritative token/cost totals for the session.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Summary {
    pub is_error: bool,
    pub result: String,
    pub total_cost_usd: f64,
    pub input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub output_tokens: u64,
    pub num_turns: u64,
    pub duration_ms: u64,
}

/// Where the live status message currently lives. It starts as the deferred
/// interaction reply and migrates to a channel message at the deadline.
enum Live {
    Interaction,
    Channel(MessageId),
}

/// Fold one NDJSON line into live `stats`, capturing the final `summary` when the
/// result envelope arrives. Malformed lines are ignored.
pub fn ingest(line: &str, stats: &mut Stats, summary: &mut Option<Summary>) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    match v.get("type").and_then(|t| t.as_str()) {
        Some("assistant") => {
            let Some(msg) = v.get("message") else { return };
            stats.steps += 1;
            if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
                for block in content {
                    if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                        if let Some(name) = block.get("name").and_then(|n| n.as_str()) {
                            stats.current_tool = Some(name.to_string());
                        }
                    }
                }
            }
            // output_tokens is per-turn; summing across turns approximates the
            // running generated-token total (good enough for a live indicator).
            stats.output_tokens += msg
                .get("usage")
                .and_then(|u| u.get("output_tokens"))
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
        }
        Some("result") => {
            let usage = v.get("usage");
            let u = |f: &str| {
                usage
                    .and_then(|x| x.get(f))
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0)
            };
            *summary = Some(Summary {
                is_error: v.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false),
                result: v
                    .get("result")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                total_cost_usd: v.get("total_cost_usd").and_then(|x| x.as_f64()).unwrap_or(0.0),
                input_tokens: u("input_tokens"),
                cache_read_input_tokens: u("cache_read_input_tokens"),
                output_tokens: u("output_tokens"),
                num_turns: v.get("num_turns").and_then(|x| x.as_u64()).unwrap_or(0),
                duration_ms: v.get("duration_ms").and_then(|x| x.as_u64()).unwrap_or(0),
            });
        }
        _ => {}
    }
}

/// The periodic progress line.
pub fn progress_text(elapsed: Duration, stats: &Stats) -> String {
    let mut s = format!(
        "⏳ working {} · {} step{} · ~{} output tok",
        human_elapsed(elapsed),
        stats.steps,
        plural(stats.steps),
        human_count(stats.output_tokens),
    );
    if let Some(tool) = &stats.current_tool {
        s.push_str(&format!(" · running {tool}"));
    }
    s
}

/// The final message: the result body plus a token/cost footer, or an error /
/// no-result note. `elapsed` is the wall-clock the runner measured.
pub fn final_text(summary: Option<&Summary>, stats: &Stats, elapsed: Duration) -> String {
    match summary {
        Some(s) if !s.is_error => {
            let body = if s.result.trim().is_empty() {
                "(claude produced no output)".to_string()
            } else {
                s.result.clone()
            };
            let footer = format!(
                "— {} turn{} · {} in / {} out · {} cache-read · ${:.4} · {}",
                s.num_turns,
                plural(s.num_turns),
                human_count(s.input_tokens),
                human_count(s.output_tokens),
                human_count(s.cache_read_input_tokens),
                s.total_cost_usd,
                human_elapsed(elapsed),
            );
            truncate_with_footer(&body, &footer)
        }
        Some(s) => {
            let msg = if s.result.trim().is_empty() {
                "claude reported an error".to_string()
            } else {
                s.result.clone()
            };
            truncate_discord(&format!("❌ claude failed after {}: {msg}", human_elapsed(elapsed)))
        }
        None => truncate_discord(&format!(
            "❓ claude ended without a result after {} ({} step{}, ~{} output tok) — it may have failed to start or crashed.",
            human_elapsed(elapsed),
            stats.steps,
            plural(stats.steps),
            human_count(stats.output_tokens),
        )),
    }
}

/// Drive the session to completion, keeping one live Discord message current.
pub async fn drive(ctx: &Context, command: &CommandInteraction, mut child: Child) {
    let start = Instant::now();
    let Some(stdout) = child.stdout.take() else {
        let text = "could not read claude output (no stdout)".to_string();
        let _ = command
            .edit_response(&ctx.http, EditInteractionResponse::new().content(text))
            .await;
        return;
    };
    let mut lines = BufReader::new(stdout).lines();

    let mut stats = Stats::default();
    let mut summary: Option<Summary> = None;
    let mut live = Live::Interaction;

    let mut tick = tokio::time::interval_at(
        tokio::time::Instant::now() + PROGRESS_INTERVAL,
        PROGRESS_INTERVAL,
    );
    let deadline = tokio::time::sleep(TRANSITION_DEADLINE);
    tokio::pin!(deadline);
    let mut transitioned = false;

    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(l)) => ingest(&l, &mut stats, &mut summary),
                _ => break, // EOF or read error → the session has ended
            },
            _ = tick.tick() => {
                let text = progress_text(start.elapsed(), &stats);
                update_live(ctx, command, &live, text).await;
            }
            _ = &mut deadline, if !transitioned => {
                transitioned = true;
                let text = progress_text(start.elapsed(), &stats);
                transition(ctx, command, &mut live, text).await;
            }
        }
    }

    let _ = child.wait().await;
    let text = final_text(summary.as_ref(), &stats, start.elapsed());
    finalize(ctx, command, &live, text).await;
}

/// Edit the current live message in place (best-effort).
async fn update_live(ctx: &Context, command: &CommandInteraction, live: &Live, text: String) {
    match live {
        Live::Interaction => {
            let _ = command
                .edit_response(&ctx.http, EditInteractionResponse::new().content(text))
                .await;
        }
        Live::Channel(id) => {
            let _ = command
                .channel_id
                .edit_message(&ctx.http, *id, EditMessage::new().content(text))
                .await;
        }
    }
}

/// Delete the deferred reply and continue in a fresh channel message that Discord
/// never expires, so long sessions keep updating past the 15-minute window.
async fn transition(ctx: &Context, command: &CommandInteraction, live: &mut Live, text: String) {
    let _ = command.delete_response(&ctx.http).await;
    match command
        .channel_id
        .send_message(&ctx.http, CreateMessage::new().content(text))
        .await
    {
        Ok(msg) => *live = Live::Channel(msg.id),
        Err(e) => eprintln!("ralphd: /btw transition to channel message failed: {e}"),
    }
}

/// Replace the live message with the final result, falling back to a new channel
/// message if the in-place edit fails.
async fn finalize(ctx: &Context, command: &CommandInteraction, live: &Live, text: String) {
    let err = match live {
        Live::Interaction => command
            .edit_response(&ctx.http, EditInteractionResponse::new().content(text.clone()))
            .await
            .err()
            .map(|e| e.to_string()),
        Live::Channel(id) => command
            .channel_id
            .edit_message(&ctx.http, *id, EditMessage::new().content(text.clone()))
            .await
            .err()
            .map(|e| e.to_string()),
    };
    if let Some(e) = err {
        eprintln!("ralphd: /btw final update failed ({e}); posting result as a new message");
        let _ = command
            .channel_id
            .send_message(&ctx.http, CreateMessage::new().content(text))
            .await;
    }
}

fn plural(n: u64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// `45s`, or `6m12s` once past a minute.
fn human_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else {
        format!("{}m{:02}s", s / 60, s % 60)
    }
}

/// Compact token counts: `500`, `1.2k`, `3.4M`.
fn human_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn truncate_discord(text: &str) -> String {
    if text.chars().count() <= DISCORD_LIMIT {
        return text.to_string();
    }
    let mut out: String = text.chars().take(DISCORD_LIMIT - 1).collect();
    out.push('…');
    out
}

/// Lay out `<body>` followed by a code-spanned `footer`, truncating the body so
/// the whole thing fits Discord's cap while always keeping the footer.
fn truncate_with_footer(body: &str, footer: &str) -> String {
    let deco = format!("\n\n`{footer}`");
    let budget = DISCORD_LIMIT.saturating_sub(deco.chars().count());
    let body = if body.chars().count() > budget {
        let mut t: String = body.chars().take(budget.saturating_sub(1)).collect();
        t.push('…');
        t
    } else {
        body.to_string()
    };
    format!("{body}{deco}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_folds_turns_tokens_and_tool() {
        let mut stats = Stats::default();
        let mut summary = None;
        ingest(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}],"usage":{"output_tokens":120}}}"#,
            &mut stats,
            &mut summary,
        );
        ingest(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"ok"}],"usage":{"output_tokens":80}}}"#,
            &mut stats,
            &mut summary,
        );
        assert_eq!(stats.steps, 2);
        assert_eq!(stats.output_tokens, 200); // summed across turns
        assert_eq!(stats.current_tool.as_deref(), Some("Bash"));
        assert!(summary.is_none());
        // malformed lines are ignored
        ingest("not json", &mut stats, &mut summary);
        assert_eq!(stats.steps, 2);
    }

    #[test]
    fn ingest_captures_result_envelope() {
        let mut stats = Stats::default();
        let mut summary = None;
        ingest(
            r#"{"type":"result","is_error":false,"result":"all done","total_cost_usd":0.0912,"num_turns":6,"duration_ms":372000,"usage":{"input_tokens":1500,"cache_read_input_tokens":120500,"output_tokens":6200}}"#,
            &mut stats,
            &mut summary,
        );
        let s = summary.expect("summary");
        assert!(!s.is_error);
        assert_eq!(s.result, "all done");
        assert_eq!(s.total_cost_usd, 0.0912);
        assert_eq!(s.num_turns, 6);
        assert_eq!(s.input_tokens, 1500);
        assert_eq!(s.cache_read_input_tokens, 120_500);
        assert_eq!(s.output_tokens, 6200);
    }

    #[test]
    fn ingest_tolerates_real_cli_result_envelope() {
        // A real `claude -p --output-format stream-json` result line (trimmed):
        // extra keys must be ignored and the totals read correctly.
        let line = r#"{"type":"result","subtype":"success","is_error":false,"api_error_status":null,"duration_ms":2613,"duration_api_ms":2558,"num_turns":1,"result":"pong","stop_reason":"end_turn","session_id":"65c16eb1","total_cost_usd":0.049923,"usage":{"input_tokens":10,"cache_creation_input_tokens":24809,"cache_read_input_tokens":0,"output_tokens":59,"cache_creation":{"ephemeral_1h_input_tokens":24809},"service_tier":"standard"},"modelUsage":{"claude-haiku-4-5":{"inputTokens":10}}}"#;
        let mut stats = Stats::default();
        let mut summary = None;
        ingest(line, &mut stats, &mut summary);
        let s = summary.expect("summary from real envelope");
        assert!(!s.is_error);
        assert_eq!(s.result, "pong");
        assert_eq!(s.total_cost_usd, 0.049923);
        assert_eq!(s.num_turns, 1);
        assert_eq!(s.input_tokens, 10);
        assert_eq!(s.output_tokens, 59);
        assert_eq!(s.cache_read_input_tokens, 0);
    }

    #[test]
    fn progress_text_reports_elapsed_turns_and_tool() {
        let stats = Stats {
            steps: 3,
            output_tokens: 4200,
            current_tool: Some("Edit".into()),
        };
        let text = progress_text(Duration::from_secs(5 * 60), &stats);
        assert!(text.contains("5m00s"), "{text}");
        assert!(text.contains("3 steps"), "{text}");
        assert!(text.contains("4.2k output tok"), "{text}");
        assert!(text.contains("running Edit"), "{text}");
    }

    #[test]
    fn final_text_success_carries_result_and_cost() {
        let summary = Summary {
            is_error: false,
            result: "the answer is 42".into(),
            total_cost_usd: 0.0912,
            input_tokens: 1500,
            cache_read_input_tokens: 120_500,
            output_tokens: 6200,
            num_turns: 6,
            duration_ms: 372_000,
        };
        let text = final_text(Some(&summary), &Stats::default(), Duration::from_secs(372));
        assert!(text.contains("the answer is 42"), "{text}");
        assert!(text.contains("$0.0912"), "{text}");
        assert!(text.contains("6 turns"), "{text}");
        assert!(text.contains("120.5k cache-read"), "{text}");
        assert!(text.contains("6m12s"), "{text}");
    }

    #[test]
    fn final_text_error_and_missing_result() {
        let err = Summary {
            is_error: true,
            result: "rate limited".into(),
            ..Summary::default()
        };
        let t = final_text(Some(&err), &Stats::default(), Duration::from_secs(30));
        assert!(t.starts_with("❌ claude failed after 30s"), "{t}");
        assert!(t.contains("rate limited"), "{t}");

        let none = final_text(None, &Stats { steps: 2, output_tokens: 10, current_tool: None }, Duration::from_secs(90));
        assert!(none.contains("without a result"), "{none}");
        assert!(none.contains("1m30s"), "{none}");
    }

    #[test]
    fn footer_is_always_kept_when_body_is_huge() {
        let body = "x".repeat(5000);
        let footer = "— 6 turns · 1.5k in / 6.2k out · $0.09";
        let out = truncate_with_footer(&body, footer);
        assert!(out.chars().count() <= DISCORD_LIMIT);
        assert!(out.contains(footer), "footer must survive truncation");
        assert!(out.contains('…'));
    }

    #[test]
    fn human_helpers_format_compactly() {
        assert_eq!(human_count(500), "500");
        assert_eq!(human_count(4200), "4.2k");
        assert_eq!(human_count(1_500_000), "1.5M");
        assert_eq!(human_elapsed(Duration::from_secs(45)), "45s");
        assert_eq!(human_elapsed(Duration::from_secs(372)), "6m12s");
    }
}
