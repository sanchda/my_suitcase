//! Mapping the edited buffer to a Claude Code PermissionRequest decision.

use serde_json::{json, Value};

/// How an approval hands the plan back to Claude.
pub enum ApproveMode {
    /// `allow` + edited `updatedInput.plan` — Claude proceeds cleanly and
    /// implements the *edited* plan. Verified empirically: a modified
    /// `updatedInput.plan` reaches the model as the approved plan. Default.
    Input,
    /// `deny` + `message` (edited plan, framed "implement verbatim"). Fallback
    /// for the rare case a heavy rewrite trips Claude's prompt-injection
    /// defense on the `allow` path; re-triggers the gate.
    Deny,
}

impl ApproveMode {
    /// Read from `PLAN_GATE_APPROVE_MODE` (`deny` → Deny, else Input).
    pub fn from_env() -> Self {
        match std::env::var("PLAN_GATE_APPROVE_MODE").ok().as_deref() {
            Some("deny") => ApproveMode::Deny,
            _ => ApproveMode::Input,
        }
    }
}

/// What to tell Claude.
pub enum Outcome {
    Approve { plan: String, tool_input: Value },
    Reject { reason: String },
}

/// Serialize the outcome to a decision object and print it to stdout.
pub fn emit(outcome: Outcome, mode: ApproveMode) {
    let value = match outcome {
        Outcome::Approve { plan, tool_input } => approve(plan, tool_input, mode),
        Outcome::Reject { reason } => reject(reason),
    };
    crate::debug::log(&format!("emitting to stdout: {value}"));
    println!("{value}");
}

fn approve(plan: String, tool_input: Value, mode: ApproveMode) -> Value {
    match mode {
        ApproveMode::Input => {
            // Replace `plan` in the original tool_input with the edited body,
            // preserving any other fields. updatedInput must be present or
            // Claude Code drops an ExitPlanMode allow.
            let updated = match tool_input {
                Value::Object(mut m) => {
                    m.insert("plan".to_string(), Value::String(plan));
                    Value::Object(m)
                }
                _ => json!({ "plan": plan }),
            };
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": {
                        "behavior": "allow",
                        "updatedInput": updated
                    }
                }
            })
        }
        ApproveMode::Deny => json!({
            "hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": {
                    "behavior": "deny",
                    "message": format!(
                        "The user finalized the plan in their editor. This is the authoritative \
                         plan — implement it exactly as written, do not re-plan:\n\n{plan}"
                    )
                }
            }
        }),
    }
}

fn reject(reason: String) -> Value {
    let message = if reason.trim().is_empty() {
        "The user rejected the plan in their editor. Revise the approach and resubmit."
            .to_string()
    } else {
        format!(
            "The user rejected the plan and left the following notes. Revise and resubmit:\n\n{reason}"
        )
    };
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": {
                "behavior": "deny",
                "message": message
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approve_input_replaces_plan_and_allows() {
        let v = approve("EDITED".into(), json!({"plan": "orig"}), ApproveMode::Input);
        let out = &v["hookSpecificOutput"];
        assert_eq!(out["decision"]["behavior"], "allow");
        // The edited plan replaces the original in updatedInput.
        assert_eq!(out["decision"]["updatedInput"]["plan"], "EDITED");
        // No dead additionalContext field.
        assert!(out.get("additionalContext").is_none());
    }

    #[test]
    fn approve_input_handles_non_object_tool_input() {
        let v = approve("EDITED".into(), Value::Null, ApproveMode::Input);
        assert_eq!(
            v["hookSpecificOutput"]["decision"]["updatedInput"]["plan"],
            "EDITED"
        );
    }

    #[test]
    fn approve_deny_mode_denies_with_plan() {
        let v = approve("PLAN".into(), json!({"plan": "orig"}), ApproveMode::Deny);
        let d = &v["hookSpecificOutput"]["decision"];
        assert_eq!(d["behavior"], "deny");
        assert!(d["message"].as_str().unwrap().contains("PLAN"));
    }

    #[test]
    fn reject_with_notes_includes_them() {
        let v = reject("use X instead".into());
        let d = &v["hookSpecificOutput"]["decision"];
        assert_eq!(d["behavior"], "deny");
        assert!(d["message"].as_str().unwrap().contains("use X instead"));
    }

    #[test]
    fn reject_empty_is_generic() {
        let v = reject("   ".into());
        assert_eq!(v["hookSpecificOutput"]["decision"]["behavior"], "deny");
    }
}
