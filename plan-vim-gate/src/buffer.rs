//! The scratch buffer: writing the directive header + plan, and parsing it back.

use crate::R;
use std::io::Write;
use tempfile::{Builder, NamedTempFile};

/// Directive header prepended to the plan. Only `decision` is meaningful;
/// the prose and any unknown `key: value` lines are ignored by the parser, so
/// the block is safe to extend later.
const HEADER: &str = "\
<!-- plan-gate
decision: approve

Edit the plan below, then save & CLOSE this pane to submit it as the final plan.
To reject: set `decision: reject` above and replace the body with your revision notes.
(Unknown keys are ignored — safe to extend.)
-->

";

const OPEN: &str = "<!-- plan-gate";
const CLOSE: &str = "-->";

/// The user's intent, parsed from the header.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Reject,
}

/// Result of parsing an edited buffer.
#[derive(Debug, PartialEq, Eq)]
pub struct Parsed {
    pub decision: Decision,
    /// Everything below the header block — the finalized plan (approve) or the
    /// revision notes (reject).
    pub body: String,
}

/// Write the header + plan to a fresh scratch `.md` file.
pub fn write_scratch(plan: &str) -> R<NamedTempFile> {
    let mut f = Builder::new().prefix("plan-gate-").suffix(".md").tempfile()?;
    f.write_all(HEADER.as_bytes())?;
    f.write_all(plan.as_bytes())?;
    if !plan.ends_with('\n') {
        f.write_all(b"\n")?;
    }
    f.flush()?;
    Ok(f)
}

/// Parse an edited buffer into a decision + body. Fail-safe: any structural
/// problem (missing header, garbled block, unrecognized decision) resolves to
/// `Reject` so the gate never auto-approves by accident.
pub fn parse(content: &str) -> Parsed {
    let trimmed = content.trim_start();

    // The header must be the very first thing in the file.
    if trimmed.starts_with(OPEN) {
        if let Some(close_at) = trimmed.find(CLOSE) {
            let header = &trimmed[OPEN.len()..close_at];
            let body = trimmed[close_at + CLOSE.len()..].trim();
            return Parsed {
                decision: parse_decision(header),
                body: body.to_string(),
            };
        }
    }

    // No usable header → reject, treating the whole content as notes.
    Parsed {
        decision: Decision::Reject,
        body: content.trim().to_string(),
    }
}

/// Scan the header block for a `decision:` line. Anything other than an
/// explicit approval word is a reject.
fn parse_decision(header: &str) -> Decision {
    for line in header.lines() {
        if let Some(rest) = line.trim().strip_prefix("decision:") {
            return match rest.trim().to_ascii_lowercase().as_str() {
                "approve" | "approved" | "accept" | "ok" | "yes" => Decision::Approve,
                _ => Decision::Reject,
            };
        }
    }
    Decision::Reject
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(decision: &str, body: &str) -> String {
        format!("<!-- plan-gate\ndecision: {decision}\n-->\n\n{body}")
    }

    #[test]
    fn approve_keeps_body() {
        let p = parse(&buf("approve", "# Plan\n\nDo the thing."));
        assert_eq!(p.decision, Decision::Approve);
        assert_eq!(p.body, "# Plan\n\nDo the thing.");
    }

    #[test]
    fn reject_keeps_body_as_notes() {
        let p = parse(&buf("reject", "Wrong approach, use X."));
        assert_eq!(p.decision, Decision::Reject);
        assert_eq!(p.body, "Wrong approach, use X.");
    }

    #[test]
    fn unrecognized_decision_is_reject() {
        assert_eq!(parse(&buf("maybe", "x")).decision, Decision::Reject);
    }

    #[test]
    fn missing_header_is_reject() {
        let p = parse("# Just a plan with no header");
        assert_eq!(p.decision, Decision::Reject);
        assert_eq!(p.body, "# Just a plan with no header");
    }

    #[test]
    fn deleted_close_marker_is_reject() {
        let p = parse("<!-- plan-gate\ndecision: approve\nno close here");
        assert_eq!(p.decision, Decision::Reject);
    }

    #[test]
    fn extra_keys_are_ignored() {
        let raw = "<!-- plan-gate\nmode: strict\ndecision: approve\nreview-with: linter\n-->\n\nbody";
        let p = parse(raw);
        assert_eq!(p.decision, Decision::Approve);
        assert_eq!(p.body, "body");
    }

    #[test]
    fn leading_whitespace_before_header_ok() {
        let p = parse("\n\n<!-- plan-gate\ndecision: approve\n-->\n\nbody");
        assert_eq!(p.decision, Decision::Approve);
        assert_eq!(p.body, "body");
    }

    #[test]
    fn roundtrip_from_written_header() {
        // The default written header approves; parsing it back should approve.
        let raw = format!("{HEADER}# Real plan");
        let p = parse(&raw);
        assert_eq!(p.decision, Decision::Approve);
        assert_eq!(p.body, "# Real plan");
    }
}
