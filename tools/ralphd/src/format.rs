//! Turn `ralph status --json` output into a compact Discord message.

use serde_json::Value;

/// Format a status JSON document plus a run-state line into a Discord message.
/// `running` is ralphd's own pid-liveness verdict (ralph does not know it).
pub fn status_message(json: &str, running: bool) -> String {
    let v: Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return format!("⚠️ could not parse ralph status output:\n```\n{json}\n```"),
    };
    let iter = v.get("iteration").and_then(Value::as_u64).unwrap_or(0);
    let pending = v
        .get("pending_leaf_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let run = if running {
        "▶️ running"
    } else {
        "⏸️ idle"
    };
    let mut out = format!("**ralph** — {run} · iter {iter} · {pending} pending\n");
    match v.get("current") {
        Some(Value::Object(c)) => {
            let label = c.get("label").and_then(Value::as_str).unwrap_or("?");
            out.push_str(&format!("**current:** {label}\n"));
        }
        _ => out.push_str("**current:** backlog complete\n"),
    }
    if let Some(Value::Array(upcoming)) = v.get("upcoming") {
        for item in upcoming {
            if let Some(s) = item.as_str() {
                out.push_str(&format!("• {s}\n"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_a_running_snapshot() {
        let json = r#"{"iteration":7,"pending_leaf_count":3,"current":{"id":"2","label":"2 — Current.","excerpt":"..."},"upcoming":["3 — Next.","4 — After."]}"#;
        let msg = status_message(json, true);
        assert!(msg.contains("running"));
        assert!(msg.contains("iter 7"));
        assert!(msg.contains("3 pending"));
        assert!(msg.contains("2 — Current."));
        assert!(msg.contains("• 3 — Next."));
    }

    #[test]
    fn formats_a_complete_backlog() {
        let json = r#"{"iteration":9,"pending_leaf_count":0,"current":null,"upcoming":[]}"#;
        let msg = status_message(json, false);
        assert!(msg.contains("idle"));
        assert!(msg.contains("backlog complete"));
    }
}
