//! Opt-in adversarial check-off judge: after a committed `code` iteration on a
//! judged model tier, a one-shot second model reads the leaf's contract and the
//! iteration's diff and tries to REFUTE the claimed completion. A refuted
//! check-off is mechanically revoked (the leaf reopens in BACKLOG) and the
//! iteration counts as no-progress.
//!
//! Missing/hung/garbled output is Unavailable; the acceptance policy decides
//! whether review availability is required. Legacy tier reviews fail open.

use crate::config::Config;
use serde::{Deserialize, Serialize};

/// The judge's decision on one check-off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Pass,
    Refuted(String),
    Unavailable,
}

/// Should this iteration be judged? Only when the feature is enabled and the
/// model that ran the iteration is one of the configured judge tiers.
pub fn wants_judgment(cfg: &Config, model: &str) -> bool {
    cfg.judge_tiers
        .iter()
        .any(|t| t == model || cfg.tier_models.get(t).is_some_and(|m| m == model))
}

/// Assemble the adversarial prompt. Kept pure for testing.
pub fn build_prompt(leaf_label: &str, leaf_excerpt: &str, summary: &str, diff: &str) -> String {
    format!(
        "You are an adversarial reviewer for an autonomous coding loop. A worker \
         requests acceptance of the task below. Your ONLY job is \
         to try to REFUTE the completion claim: does the diff plausibly satisfy the \
         task and its `Verify:` contract? Judge from the evidence given — do not \
         assume unverified claims in the summary are true. If the diff is clearly \
         unrelated, trivially cosmetic, or obviously incomplete against the \
         contract, refute. Qualitative criteria are valid: use the stated constraints \
         and examples, not an invented binary proxy. Work may already exist; an empty \
         diff alone is not grounds for refutation. Inspect narrowly relevant project \
         files if needed. If you are uncertain, refute.\n\n\
         Reply with EXACTLY one first line: `PASS` or `REFUTE: <one-line reason>`. \
         No other output before it.\n\n\
         ## Task (as checked off)\n{leaf_label}\n\n{leaf_excerpt}\n\n\
         ## Worker's end-of-turn summary (claims, unverified)\n{summary}\n\n\
         ## The iteration's commits and diff\n{diff}\n"
    )
}

/// Parse the judge's raw output without treating unavailability as a pass.
pub fn parse_decision(raw: &str) -> Decision {
    let first = raw.trim().lines().next().unwrap_or("").trim();
    if let Some(reason) = first.strip_prefix("REFUTE") {
        let reason = reason.trim_start_matches(':').trim();
        return Decision::Refuted(if reason.is_empty() {
            "no reason given".to_string()
        } else {
            reason.to_string()
        });
    }
    if first == "PASS" {
        Decision::Pass
    } else {
        Decision::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_carries_contract_summary_and_diff() {
        let p = build_prompt(
            "3.1 — Wire the parser.",
            "- [x] **3.1 — Wire the parser.** Verify: cargo test parser",
            "wired it up, tests green",
            "diff --git a/src/parser.rs …",
        );
        assert!(p.contains("3.1 — Wire the parser."));
        assert!(p.contains("Verify: cargo test parser"));
        assert!(p.contains("tests green"));
        assert!(p.contains("a/src/parser.rs"));
        assert!(p.contains("REFUTE"));
    }

    #[test]
    fn decision_parsing_distinguishes_unavailable_from_pass() {
        assert_eq!(parse_decision("PASS"), Decision::Pass);
        assert_eq!(parse_decision("  PASS\nextra prose"), Decision::Pass);
        assert_eq!(
            parse_decision("REFUTE: diff touches only README"),
            Decision::Refuted("diff touches only README".into())
        );
        assert_eq!(
            parse_decision("REFUTE"),
            Decision::Refuted("no reason given".into())
        );
        // Garbled output must not stall the loop.
        assert_eq!(parse_decision(""), Decision::Unavailable);
        assert_eq!(
            parse_decision("I think this is fine"),
            Decision::Unavailable
        );
        // But a second-line REFUTE after a chatty first line does NOT count —
        // the contract is first-line-only.
        assert_eq!(parse_decision("Well.\nREFUTE: late"), Decision::Unavailable);
    }

    #[test]
    fn wants_judgment_matches_configured_tiers() {
        let cfg = Config {
            judge_tiers: vec!["opus".into()],
            ..Config::default()
        };
        assert!(wants_judgment(&cfg, "opus"));
        assert!(!wants_judgment(&cfg, "sonnet"));
        assert!(!wants_judgment(&Config::default(), "opus")); // off by default
    }
}
