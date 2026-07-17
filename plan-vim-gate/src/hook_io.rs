//! Reading the Claude Code hook event from stdin.

use crate::R;
use serde_json::Value;
use std::io::Read;

/// The parsed ExitPlanMode hook event, holding what the gate needs.
pub struct HookEvent {
    /// The original `tool_input` object, echoed back verbatim as `updatedInput`
    /// on an allow decision (Claude Code drops an ExitPlanMode allow without it).
    pub tool_input: Value,
    /// The plan markdown (`.tool_input.plan`).
    pub plan: String,
}

/// Read the entire hook event JSON from stdin and extract the plan.
pub fn read_event() -> R<HookEvent> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let raw: Value = serde_json::from_str(&buf)?;

    let tool_input = raw.get("tool_input").cloned().unwrap_or(Value::Null);
    let plan = tool_input
        .get("plan")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    Ok(HookEvent { tool_input, plan })
}
