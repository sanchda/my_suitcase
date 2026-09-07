//! The live status card: one pinned, edited message per loop run instead of a
//! scroll of status posts. A background task per loop keeps its card current
//! while that loop runs; on exit it gets a final past-tense edit and stays as
//! the run's record. The next run deletes it (which also unpins it) and posts a
//! fresh one, so there is only ever one card per channel.

use crate::config::LoopConfig;
use crate::handler::LoopChild;
use crate::ledger::{self, Budget, Spend};
use crate::ralph::Ralph;
use crate::{format, loop_pid};

use serenity::all::{
    ChannelId, CreateAllowedMentions, CreateMessage, EditMessage, Http, MessageId,
};
use std::sync::Arc;
use std::time::Duration;

const CARD_POLL: Duration = Duration::from_secs(30);

/// `running_pid` is None only for the closing edit. Kept pure so it is testable
/// without a gateway.
pub fn card_text(
    name: &str,
    status_json: &str,
    running_pid: Option<u32>,
    live_line: Option<&str>,
    spend: Option<Spend>,
    budget: Option<Budget>,
    now_unix: u64,
) -> String {
    let mut out = match running_pid {
        Some(pid) => format!("📌 **{name}** (pid {pid})\n"),
        None => format!("📌 **{name}** — ended\n"),
    };
    out.push_str(&format::status_message(status_json, running_pid.is_some()));
    if let Some(line) = live_line.map(str::trim).filter(|l| !l.is_empty()) {
        if running_pid.is_some() {
            out.push_str(&format!("`{line}`\n"));
        }
    }
    if let Some(line) = ledger::spend_line(spend, budget) {
        out.push_str(&line);
    }
    out.push_str(&format!("-# updated <t:{now_unix}:R>"));
    out
}

/// First line of `.ralph/live` (the in-iteration tool/elapsed/tokens line).
fn live_line(lc: &LoopConfig) -> Option<String> {
    std::fs::read_to_string(lc.state_dir.join("live"))
        .ok()
        .and_then(|s| s.lines().next().map(str::to_string))
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Background task: maintain one loop's card for the ralphd process lifetime.
pub async fn watch_card(lc: LoopConfig, loop_child: LoopChild, http: Arc<Http>) {
    let channel = ChannelId::new(lc.channel_id);
    let mut card: Option<MessageId> = None;
    let mut was_running = false;

    loop {
        tokio::time::sleep(CARD_POLL).await;
        crate::handler::reap_finished(&lc, &loop_child);
        let running = loop_pid::running(&lc.state_dir);

        if running.is_none() {
            // One closing edit when the loop just ended; then leave the card be.
            if was_running {
                was_running = false;
                if let Some(id) = card {
                    let text = render(&lc, None).await;
                    let _ = channel
                        .edit_message(&http, id, EditMessage::new().content(text))
                        .await;
                }
            }
            continue;
        }

        let text = render(&lc, running).await;
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

async fn render(lc: &LoopConfig, running: Option<u32>) -> String {
    let status = Ralph::new(lc)
        .status_json()
        .await
        .ok()
        .filter(|o| !o.stdout.trim().is_empty())
        .map(|o| o.stdout)
        .unwrap_or_else(|| "{}".to_string());
    let now = now_unix();
    // Budget is re-read each tick so editing ralph.toml takes effect without a
    // ralphd restart; enforcement is ralph's, this is only the warning.
    let budget = ledger::budget(&lc.ralph_config);
    let spend = ledger::read(&lc.state_dir, budget.map(|b| b.window).unwrap_or(0), now);
    card_text(
        &lc.name,
        &status,
        running,
        live_line(lc).as_deref(),
        spend,
        budget,
        now,
    )
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
    fn running_card_has_name_pid_live_line_and_timestamp() {
        let text = card_text(
            "number-grove",
            STATUS,
            Some(4242),
            Some("iter 7 | model sonnet | elapsed 3m02s"),
            None,
            None,
            1_700_000_000,
        );
        assert!(text.contains("number-grove"), "{text}");
        assert!(text.contains("pid 4242"), "{text}");
        assert!(text.contains("running"), "{text}");
        assert!(text.contains("2 — Current."), "{text}");
        assert!(text.contains("elapsed 3m02s"), "{text}");
        assert!(text.contains("<t:1700000000:R>"), "{text}");
    }

    #[test]
    fn ended_card_drops_live_line_and_reads_past_tense() {
        let text = card_text(
            "grove",
            STATUS,
            None,
            Some("iter 7 | stale"),
            None,
            None,
            1_700_000_000,
        );
        assert!(text.contains("ended"), "{text}");
        assert!(text.contains("idle"), "{text}");
        assert!(
            !text.contains("stale"),
            "stale live line must not survive the end: {text}"
        );
    }

    #[test]
    fn spend_reaches_the_card_and_warns_near_the_budget() {
        let spend = Some(Spend {
            total_usd: 8.5,
            entries: 12,
        });
        let budget = Some(Budget {
            usd: 10.0,
            window: 0,
        });
        let text = card_text("grove", STATUS, Some(1), None, spend, budget, 1_700_000_000);
        assert!(text.contains("⚠️"), "{text}");
        assert!(text.contains("$8.50 / $10.00"), "{text}");
        // A ralph too old to write a ledger simply contributes no line.
        let bare = card_text("grove", STATUS, Some(1), None, None, budget, 1_700_000_000);
        assert!(!bare.contains("spend"), "{bare}");
    }
}
