//! Optional Discord webhook notifications for loop lifecycle events.
//!
//! Best-effort by design: `DISCORD_WEBHOOK` holds a Discord **webhook URL**
//! (already bound to a channel — no channel id needed). Posts go out via `curl`
//! with a short timeout and all errors swallowed, so a down or slow webhook
//! never stalls or fails the loop.

use std::process::{Command, Stdio};

/// Posts short status lines to a Discord webhook URL.
pub struct Notifier {
    webhook: String,
}

impl Notifier {
    /// `None` when no webhook URL is configured — the feature is simply off.
    pub fn new(webhook: &str) -> Option<Self> {
        let trimmed = webhook.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(Notifier {
                webhook: trimmed.to_string(),
            })
        }
    }

    /// POST `content` to the webhook. Discord caps message content at 2000
    /// chars, so we truncate defensively; `serde_json` handles escaping.
    pub fn post(&self, content: &str) {
        let mut body: String = content.chars().take(1900).collect();
        if content.chars().count() > 1900 {
            body.push('…');
        }
        let payload = serde_json::json!({ "content": body }).to_string();
        let _ = Command::new("curl")
            .args([
                "-sS",
                "--max-time",
                "10",
                "-X",
                "POST",
                "-H",
                "Content-Type: application/json",
                "-d",
                &payload,
                &self.webhook,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Post to the notifier if one is configured; a no-op otherwise.
pub fn notify(notifier: &Option<Notifier>, content: &str) {
    if let Some(n) = notifier {
        n.post(content);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_without_webhook() {
        assert!(Notifier::new("").is_none());
        assert!(Notifier::new("   ").is_none());
    }

    #[test]
    fn enabled_with_webhook() {
        assert!(Notifier::new("https://discord.com/api/webhooks/1/abc").is_some());
    }
}
