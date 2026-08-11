//! Opt-in adversarial check-off judge: after a committed `code` iteration on a
//! judged model tier, a one-shot second model reads the leaf's contract and the
//! iteration's diff and tries to REFUTE the claimed completion. A refuted
//! check-off is mechanically revoked (the leaf reopens in BACKLOG) and the
//! iteration counts as no-progress.
//!
//! Fail-open: a missing/hung/garbled judge call passes the iteration rather
//! than stalling the loop; the prompt itself tells the judge to refute when
//! uncertain.

use crate::config::Config;
use crate::{backlog_edit, git, synth};
use std::path::Path;

/// Wall-clock cap for the one-shot judge call; larger than synth's because the
/// judge reads a full diff.
const JUDGE_TIMEOUT_SECS: u64 = 180;
/// Byte bound on the diff fed to the judge.
const JUDGE_DIFF_BYTES: usize = 24 * 1024;
/// Byte bound on the leaf excerpt fed to the judge.
const JUDGE_LEAF_BYTES: usize = 4 * 1024;

/// The judge's decision on one check-off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Pass,
    Refuted(String),
}

/// Should this iteration be judged? Only when the feature is enabled and the
/// model that ran the iteration is one of the configured judge tiers.
pub fn wants_judgment(cfg: &Config, model: &str) -> bool {
    cfg.judge_tiers.iter().any(|t| t == model)
}

/// Assemble the adversarial prompt. Kept pure for testing.
pub fn build_prompt(leaf_label: &str, leaf_excerpt: &str, summary: &str, diff: &str) -> String {
    format!(
        "You are an adversarial reviewer for an autonomous coding loop. A worker \
         iteration just checked off the task below and committed. Your ONLY job is \
         to try to REFUTE the completion claim: does the diff plausibly satisfy the \
         task and its `Verify:` contract? Judge from the evidence given — do not \
         assume unverified claims in the summary are true. If the diff is clearly \
         unrelated, trivially cosmetic, or obviously incomplete against the \
         contract, refute. If you are uncertain, refute.\n\n\
         Reply with EXACTLY one first line: `PASS` or `REFUTE: <one-line reason>`. \
         No other output before it.\n\n\
         ## Task (as checked off)\n{leaf_label}\n\n{leaf_excerpt}\n\n\
         ## Worker's end-of-turn summary (claims, unverified)\n{summary}\n\n\
         ## The iteration's commits and diff\n{diff}\n"
    )
}

/// Parse the judge's raw output. Unknown/absent verdicts pass (fail-open).
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
    Decision::Pass
}

/// Run the judge over the iteration that moved `head_before` → HEAD and, on
/// refutation, reopen the leaf in the backlog. Returns the refutation reason
/// when the check-off was revoked, `None` when the iteration stands. Best-effort
/// throughout: any missing input passes.
pub fn judge_iteration(
    cfg: &Config,
    repo: &Path,
    head_before: &Option<String>,
    leaf_id: &str,
    leaf_title: &str,
    summary: &str,
) -> Option<String> {
    let before = head_before.as_deref()?;
    let now = git::head(repo)?;
    let diff = git::range_diff_text(repo, before, &now, JUDGE_DIFF_BYTES);
    if diff.trim().is_empty() {
        return None;
    }
    // The leaf is still in the live backlog here (curation runs later); pull its
    // own prose so the judge sees the Verify contract.
    let text = std::fs::read_to_string(&cfg.backlog).ok()?;
    let doc = crate::backlog::Document::parse(&text);
    let index = doc.tasks.iter().position(|t| t.id == leaf_id)?;
    let excerpt = doc.own_excerpt(index, JUDGE_LEAF_BYTES);

    let label = format!("{leaf_id} — {leaf_title}");
    let prompt = build_prompt(&label, &excerpt, summary, &diff);
    let raw = synth::run_claude_oneshot(&cfg.judge_model, JUDGE_TIMEOUT_SECS, &prompt)?;
    match parse_decision(&raw) {
        Decision::Pass => None,
        Decision::Refuted(reason) => {
            // A failed reopen leaves the backlog untouched; the refutation
            // still counts as no-progress via the streak.
            if let Ok(new_text) = backlog_edit::apply_uncheck(&text, leaf_id) {
                let _ = backlog_edit::write_atomic(&cfg.backlog, &new_text);
            }
            Some(reason)
        }
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
    fn decision_parsing_is_first_line_only_and_fails_open() {
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
        assert_eq!(parse_decision(""), Decision::Pass);
        assert_eq!(parse_decision("I think this is fine"), Decision::Pass);
        // But a second-line REFUTE after a chatty first line does NOT count —
        // the contract is first-line-only.
        assert_eq!(parse_decision("Well.\nREFUTE: late"), Decision::Pass);
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
