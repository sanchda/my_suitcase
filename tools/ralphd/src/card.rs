//! The live status card: one pinned, edited message per loop run instead of a
//! scroll of status posts. A background task keeps it current while the loop
//! runs; on exit it gets a final past-tense edit and stays as the run's
//! record. The next run deletes it (which also unpins it) and posts a fresh
//! one, so there is only ever one card.

use crate::config::BotConfig;
use crate::handler::LoopChild;
use crate::ralph::Ralph;
use crate::{format, loop_pid};

use serenity::all::{ChannelId, CreateAllowedMentions, CreateMessage, EditMessage, Http, MessageId};
use std::sync::Arc;
use std::time::Duration;

/// Cadence of card refreshes while a loop is running.
const CARD_POLL: Duration = Duration::from_secs(30);

/// Compose the card body. `running_pid` is Some while the loop lives; the
/// closing edit passes None. Pure for testing.
pub fn card_text(status_json: &str, running_pid: Option<u32>, live_line: Option<&str>, now_unix: u64) -> String {
    let mut out = match running_pid {
        Some(pid) => format!("📌 **ralph loop** (pid {pid})\n"),
        None => "📌 **ralph loop** — ended\n".to_string(),
    };
    out.push_str(&format::status_message(status_json, running_pid.is_some()));
    if let Some(line) = live_line.map(str::trim).filter(|l| !l.is_empty()) {
        if running_pid.is_some() {
            out.push_str(&format!("`{line}`\n"));
        }
    }
    out.push_str(&format!("-# updated <t:{now_unix}:R>"));
    out
}

/// First line of `.ralph/live` (the in-iteration tool/elapsed/tokens line).
fn live_line(cfg: &BotConfig) -> Option<String> {
    std::fs::read_to_string(cfg.state_dir.join("live"))
        .ok()
        .and_then(|s| s.lines().next().map(str::to_string))
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Background task: maintain the card for the ralphd process lifetime.
pub async fn watch_card(cfg: BotConfig, loop_child: LoopChild, http: Arc<Http>) {
    let channel = ChannelId::new(cfg.channel_id);
    let mut card: Option<MessageId> = None;
    let mut was_running = false;

    loop {
        tokio::time::sleep(CARD_POLL).await;
        crate::handler::reap_and_clear(&cfg, &loop_child);
        let running = loop_pid::running(&cfg.state_dir);

        if running.is_none() {
            // One closing edit when the loop just ended; then leave the card be.
            if was_running {
                was_running = false;
                if let Some(id) = card {
                    let text = render(&cfg, None);
                    let _ = channel
                        .edit_message(&http, id, EditMessage::new().content(text))
                        .await;
                }
            }
            continue;
        }

        let text = render(&cfg, running);
        match card {
            None => card = post_fresh(&http, channel, &text, None).await,
            Some(id) => {
                if !was_running {
                    // New run: replace the previous run's card.
                    card = post_fresh(&http, channel, &text, Some(id)).await;
                } else if channel
                    .edit_message(&http, id, EditMessage::new().content(text.clone()))
                    .await
                    .is_err()
                {
                    // Card was deleted out from under us — recreate.
                    card = post_fresh(&http, channel, &text, None).await;
                }
            }
        }
        was_running = true;
    }
}

fn render(cfg: &BotConfig, running: Option<u32>) -> String {
    let status = Ralph::new(cfg)
        .status_json()
        .ok()
        .filter(|o| o.ok)
        .map(|o| o.stdout)
        .unwrap_or_else(|| "{}".to_string());
    card_text(&status, running, live_line(cfg).as_deref(), now_unix())
}

/// Post a new card, pin it best-effort (pinning needs Manage Messages; a failed
/// pin degrades to an ordinary message), deleting `old` first when given.
async fn post_fresh(
    http: &Arc<Http>,
    channel: ChannelId,
    text: &str,
    old: Option<MessageId>,
) -> Option<MessageId> {
    if let Some(id) = old {
        let _ = channel.delete_message(http, id).await;
    }
    match channel
        .send_message(
            http,
            CreateMessage::new()
                .content(text)
                .allowed_mentions(CreateAllowedMentions::new()),
        )
        .await
    {
        Ok(msg) => {
            let _ = msg.pin(http).await;
            Some(msg.id)
        }
        Err(e) => {
            eprintln!("ralphd: could not post status card: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = r#"{"iteration":7,"pending_leaf_count":3,"current":{"id":"2","label":"2 — Current."},"upcoming":["3 — Next."]}"#;

    #[test]
    fn running_card_has_pid_live_line_and_timestamp() {
        let text = card_text(STATUS, Some(4242), Some("iter 7 | model sonnet | elapsed 3m02s"), 1_700_000_000);
        assert!(text.contains("pid 4242"), "{text}");
        assert!(text.contains("running"), "{text}");
        assert!(text.contains("2 — Current."), "{text}");
        assert!(text.contains("elapsed 3m02s"), "{text}");
        assert!(text.contains("<t:1700000000:R>"), "{text}");
    }

    #[test]
    fn ended_card_drops_live_line_and_reads_past_tense() {
        let text = card_text(STATUS, None, Some("iter 7 | stale"), 1_700_000_000);
        assert!(text.contains("ended"), "{text}");
        assert!(text.contains("idle"), "{text}");
        assert!(!text.contains("stale"), "stale live line must not survive the end: {text}");
    }
}
