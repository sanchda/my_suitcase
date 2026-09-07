//! Turn `ralph status --json` output into a compact Discord message.

use serde_json::Value;

/// Retains pid-liveness fallback for older Ralph binaries; newer snapshots also
/// carry the runner's lifecycle phase and terminal reason.
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
    if let Some(record) = v.get("run").and_then(Value::as_object) {
        if let Some(phase) = record.get("phase").and_then(Value::as_str) {
            out.push_str(&format!("**phase:** {phase}"));
            if let Some(attempts) = record.get("task_attempts").and_then(Value::as_u64) {
                out.push_str(&format!(" · {attempts} task attempts"));
            }
            out.push('\n');
        }
        if let Some(reason) = record.get("terminal_reason").and_then(Value::as_str) {
            out.push_str(&format!("**outcome:** {reason}\n"));
        }
    }
    if let Some(diagnostics) = v.get("diagnostics").and_then(Value::as_array) {
        for detail in diagnostics.iter().filter_map(Value::as_str).take(3) {
            out.push_str(&format!("⚠️ {detail}\n"));
        }
    }
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

    #[test]
    fn shows_runner_phase_and_terminal_reason() {
        let json = r#"{"iteration":3,"run":{"phase":"stopped","terminal_reason":"required review unavailable","task_attempts":2}}"#;
        let msg = status_message(json, false);
        assert!(msg.contains("required review unavailable"));
        assert!(msg.contains("2 task attempts"));
    }
}
