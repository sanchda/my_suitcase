//! Prompt construction. Pure and tested; the subprocess lives in `runner`.

use crate::manifest::{Item, Persona};

/// Cap on a voter's research brief. The brief is re-sent with every ballot, so
/// it is the one thing that multiplies across the whole run — and a bloated
/// brief dilutes the two items actually under judgment.
pub const BRIEF_WORD_CAP: usize = 250;

pub fn build_bootstrap_prompt(persona: &Persona) -> String {
    format!(
        "You are about to join an evaluation panel as one judge among several. \
         Every judge is scored against the same criterion but brings a different \
         evidence base. Yours is below. Research it now.\n\n\
         ## Your lens\n{lens}\n\n\
         ## Your research task\n{task}\n\n\
         ## Report back\n\
         At most {cap} words of CONCLUSIONS — what you found and what it implies \
         for judging quality. No file dumps, no quoted source, no narration of \
         how you searched.\n\n\
         Do NOT look for any ranking, plan, prioritization, roadmap or ordering \
         document, and do not try to find the list of items you will be judging. \
         Your entire value to the panel is an evidence base nobody else has; \
         locating someone else's answer destroys it and silently collapses the \
         panel into one opinion.\n",
        lens = persona.lens.trim(),
        task = persona.bootstrap.trim(),
        cap = BRIEF_WORD_CAP,
    )
}

/// One ballot: a fixed criterion and a batch of pairs, each already oriented.
/// `batch` is `(a, b)` as the voter will see them — orientation is applied by
/// the caller so this function never decides position.
///
/// Everything before the pairs is identical across all of one voter's ballots,
/// and that ordering is load-bearing: a stable prefix is cached by the backend,
/// so a large pre-context is paid for once and read back at a fraction of the
/// cost on every subsequent ballot. Putting the pairs first would make the
/// prefix vary and throw that away.
pub fn build_ballot_prompt(
    criterion: &str,
    preamble: &str,
    persona: &Persona,
    context: &str,
    batch: &[(&Item, &Item)],
) -> String {
    let mut p = String::new();
    p.push_str(
        "You are one judge on an evaluation panel. You are shown pairs of items \
         and pick one from each pair.\n\n",
    );
    p.push_str("## Your lens\n");
    p.push_str(persona.lens.trim());
    p.push_str("\n\n");
    if !context.trim().is_empty() {
        p.push_str(
            "## Your evidence base\n\
             Material only you on this panel have read. It is what you judge *from*, \
             never what you judge: none of it is an item, and nothing in it ranks, \
             orders or prioritizes the items for you. Where it is silent on a pair, \
             say so in your reason rather than reaching for general knowledge.\n\n",
        );
        p.push_str(context.trim());
        p.push_str("\n\n");
    }
    if !persona.brief.trim().is_empty() {
        p.push_str("## What you established researching that lens\n");
        p.push_str(persona.brief.trim());
        p.push_str("\n\n");
    }
    if !preamble.trim().is_empty() {
        p.push_str("## Definitions, shared by every item\n");
        p.push_str(preamble.trim());
        p.push_str("\n\n");
    }
    p.push_str("## The criterion — identical for every judge and every pair\n");
    p.push_str(criterion.trim());
    p.push_str("\n\n## Pairs\n");
    for (n, (a, b)) in batch.iter().enumerate() {
        p.push_str(&format!(
            "\n### {}\nA. {}\n\nB. {}\n",
            n + 1,
            a.text.trim(),
            b.text.trim()
        ));
    }
    p.push_str(&format!(
        "\n## Output\n\
         Exactly {n} lines, one per pair, in order, and nothing else:\n\n\
         `<pair number>|<A or B>|<confidence 0.00-1.00>|<one sentence, max 25 words>`\n\n\
         Example: `1|B|0.70|B's failure mode is silent, A's is loud and caught in CI.`\n\n\
         No preamble, no headings, no blank lines, no closing summary.\n\n\
         Choose for every pair. No ties, no abstentions, no \"both\" — a forced \
         choice is what keeps the comparison total.\n\n\
         Judge each pair strictly on its own merits against the criterion. Do NOT \
         try to keep your answers consistent with one another, and do NOT infer a \
         global ranking across the pairs. Transitivity is not your job; if your \
         answers imply a cycle, that is a finding the panel wants, not a mistake \
         to correct.\n\n\
         Your evidence will not cover every pair. When it is silent on one side, \
         say so plainly in your reason — the panel counts that, and a judge who \
         dresses up a guess as a finding corrupts the result more than one who \
         admits the gap.\n",
        n = batch.len()
    ));
    p
}

/// Trim a bootstrap reply to the word cap. Over-long briefs are the one input
/// that multiplies across every ballot in the run.
pub fn cap_brief(raw: &str) -> String {
    let words: Vec<&str> = raw.split_whitespace().collect();
    if words.len() <= BRIEF_WORD_CAP {
        return raw.trim().to_string();
    }
    words[..BRIEF_WORD_CAP].join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn persona(brief: &str) -> Persona {
        Persona {
            id: "v1".into(),
            lens: "storage subsystem".into(),
            context: Vec::new(),
            bootstrap: "read the storage benchmarks".into(),
            brief: brief.into(),
            model: None,
        }
    }

    fn item(id: &str, text: &str) -> Item {
        Item {
            id: id.into(),
            text: text.into(),
        }
    }

    #[test]
    fn bootstrap_forbids_hunting_for_the_source() {
        let p = build_bootstrap_prompt(&persona(""));
        assert!(p.contains("storage benchmarks"));
        assert!(p.contains("Do NOT look for any ranking"));
        assert!(p.contains("250 words"));
    }

    #[test]
    fn ballot_carries_criterion_lens_and_every_pair() {
        let (a, b, c, d) = (
            item("a1", "rewrite the allocator"),
            item("a2", "add a cache"),
            item("a3", "delete dead code"),
            item("a4", "ship the CLI"),
        );
        let p = build_ballot_prompt("which is riskier to defer", "", &persona("disks are slow"), "", &[(&a, &b), (&c, &d)]);
        assert!(p.contains("which is riskier to defer"));
        assert!(p.contains("storage subsystem"));
        assert!(p.contains("disks are slow"));
        for text in ["rewrite the allocator", "add a cache", "delete dead code", "ship the CLI"] {
            assert!(p.contains(text), "missing {text}");
        }
        assert!(p.contains("Exactly 2 lines"));
    }

    #[test]
    fn ballot_counters_the_consistency_pressure_batching_creates() {
        // Batching is the whole cost saving, and its one risk is that a voter
        // seeing several pairs at once starts building a global ranking.
        let (a, b) = (item("a1", "x"), item("a2", "y"));
        let p = build_ballot_prompt("c", "", &persona(""), "", &[(&a, &b)]);
        assert!(p.contains("Do NOT"));
        assert!(p.contains("global ranking"));
        assert!(p.contains("cycle"));
    }

    #[test]
    fn an_empty_brief_omits_its_whole_section() {
        let (a, b) = (item("a1", "x"), item("a2", "y"));
        let p = build_ballot_prompt("c", "", &persona(""), "", &[(&a, &b)]);
        assert!(!p.contains("What you established"));
        assert!(!p.contains("Your evidence base"));
    }

    #[test]
    fn pre_context_precedes_the_pairs_so_the_prefix_stays_cacheable() {
        let (a, b) = (item("a1", "x"), item("a2", "y"));
        let p = build_ballot_prompt("c", "", &persona(""), "disk latency p99 is 40ms", &[(&a, &b)]);
        let ctx = p.find("disk latency").expect("context present");
        let pairs = p.find("## Pairs").expect("pairs present");
        assert!(ctx < pairs, "context must sit in the stable prefix");
    }

    #[test]
    fn a_shared_preamble_is_sent_once_and_sits_in_the_cacheable_prefix() {
        let (a, b) = (item("a1", "x"), item("a2", "y"));
        let p = build_ballot_prompt("c", "A *widget* is a unit of work.", &persona(""), "", &[(&a, &b)]);
        assert_eq!(p.matches("A *widget* is a unit of work.").count(), 1);
        assert!(p.find("widget").unwrap() < p.find("## Pairs").unwrap());
    }

    #[test]
    fn voters_are_told_to_declare_when_their_evidence_is_silent() {
        let (a, b) = (item("a1", "x"), item("a2", "y"));
        let p = build_ballot_prompt("c", "", &persona(""), "", &[(&a, &b)]);
        assert!(p.contains("silent on one side"));
    }

    #[test]
    fn pre_context_is_framed_as_evidence_not_as_items() {
        let (a, b) = (item("a1", "x"), item("a2", "y"));
        let p = build_ballot_prompt("c", "", &persona(""), "some material", &[(&a, &b)]);
        assert!(p.contains("judge *from*, never what you judge"));
        assert!(p.contains("none of it is an item"));
    }

    #[test]
    fn briefs_are_capped_at_the_word_limit() {
        let long = "word ".repeat(400);
        assert_eq!(cap_brief(&long).split_whitespace().count(), BRIEF_WORD_CAP);
        assert_eq!(cap_brief("  short brief  "), "short brief");
    }
}
