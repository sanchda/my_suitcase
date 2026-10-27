//! The analyst's output: the items to rank and the panel that ranks them.
//!
//! The criterion is deliberately NOT part of this file. It is written by the
//! orchestrator before the analyst ever reads the source, and is passed
//! separately via `--criterion`. A manifest that carries a criterion means the
//! analyst authored it from the document under evaluation, which is exactly the
//! leak the separation of duties exists to prevent — so we reject it.

use crate::R;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Item {
    pub id: String,
    /// Verbatim from the source. Paraphrase is reframing; reframing is the
    /// orchestrator's priors entering through the back door.
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Persona {
    pub id: String,
    /// The evidence slice this voter judges from. Not an objective — every
    /// voter is scored against the same criterion (see `estimator` in the skill).
    pub lens: String,
    /// Files this voter reads before judging anything. This is the primary way
    /// to make voters differ: handing each a different slice of real material
    /// decorrelates them far more reliably than persona wording, and unlike a
    /// research call it is deterministic, auditable and cheap — the text is a
    /// stable prompt prefix, so every ballot after the first reads it from
    /// cache. Non-overlapping slices decorrelate best.
    #[serde(default)]
    pub context: Vec<String>,
    /// What this voter researches once, before any pair. Empty skips bootstrap.
    #[serde(default)]
    pub bootstrap: String,
    /// Conclusions from that research, capped. Filled in by `panel run`; may be
    /// pre-supplied to skip the bootstrap phase.
    #[serde(default)]
    pub brief: String,
    /// Backend/model for this voter — "opus", "sonnet", "codex", a concrete id.
    /// Spreading a panel across providers decorrelates errors far harder than
    /// persona wording does. None uses the CLI default.
    #[serde(default)]
    pub model: Option<String>,
}

/// A candidate the analyst saw and left out of `items`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Dropped {
    pub id: String,
    #[serde(default)]
    pub text: String,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub items: Vec<Item>,
    /// Shared definitions, emitted once per ballot instead of being duplicated
    /// into every item. Text common to both sides of a pair is pure dilution:
    /// it cannot inform the comparison, it just pushes the two things actually
    /// being compared further apart.
    #[serde(default)]
    pub preamble: String,
    /// Every candidate the analyst saw and excluded. **Required**, including as
    /// an empty list, which is the analyst asserting it dropped nothing.
    ///
    /// The absence of this field is the failure it exists to prevent: an
    /// analyst facing more candidates than the quadratic affords will quietly
    /// keep "the important ones", which is a priority judgment made by the one
    /// component that read the source — the precise contamination the whole
    /// separation of duties exists to stop, and invisible to everything
    /// downstream.
    pub dropped: Vec<Dropped>,
    pub personae: Vec<Persona>,
}

impl Manifest {
    pub fn load(path: &str) -> R<Self> {
        let raw = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let value: Value = serde_json::from_str(&raw).map_err(|e| format!("{path}: {e}"))?;
        if value.get("criterion").is_some() {
            return Err(format!(
                "{path} carries a `criterion` field. The criterion must be written \
                 before the analyst reads the source and passed with --criterion; \
                 one derived from the document is not source-independent."
            )
            .into());
        }
        if value.get("dropped").is_none() {
            return Err(format!(
                "{path} has no `dropped` field. The analyst must list every \
                 candidate it saw and excluded, or state `\"dropped\": []` to \
                 assert it excluded none.\n\n\
                 Silent subsetting is the contamination this design exists to \
                 prevent: facing more candidates than the quadratic affords, an \
                 analyst will keep \"the important ones\" — a priority judgment, \
                 made by the only component that read the source, which nothing \
                 downstream can see or check."
            )
            .into());
        }
        let manifest: Manifest = serde_json::from_value(value).map_err(|e| format!("{path}: {e}"))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> R<()> {
        if self.items.len() < 2 {
            return Err(format!("need at least 2 items, got {}", self.items.len()).into());
        }
        if self.personae.len() < 3 {
            return Err(format!("need at least 3 voters, got {}", self.personae.len()).into());
        }
        if self.personae.len() % 2 == 0 {
            return Err(format!(
                "panel size must be odd (got {}): an even panel ties, and a tied \
                 pair contributes no edge to the Copeland graph",
                self.personae.len()
            )
            .into());
        }
        dup_check(
            "item",
            self.items
                .iter()
                .map(|i| i.id.as_str())
                .chain(self.dropped.iter().map(|d| d.id.as_str())),
        )?;
        dup_check("voter", self.personae.iter().map(|p| p.id.as_str()))?;
        if let Some(item) = self.items.iter().find(|i| i.text.trim().is_empty()) {
            return Err(format!("item {} has empty text", item.id).into());
        }
        Ok(())
    }

    /// Flag a panel whose voters sit at different capability levels.
    ///
    /// This is the one composition error the design cannot survive. The
    /// Condorcet lift assumes voters of *comparable* accuracy, because simple
    /// majority is only the right aggregator when accuracies are roughly equal
    /// (the optimum otherwise is log-odds weighting, and you do not know the
    /// odds in advance). With a simple majority, a weak bloc does not merely add
    /// noise — it outvotes the strong voters. Five independent judges at
    /// p=[0.90, 0.60, 0.52, 0.52, 0.52] land at 0.71, well below the 0.90 you
    /// would have had by just asking the strong one.
    ///
    /// Mixing *providers* at one level is encouraged — claude/codex, codex/codex
    /// and claude/claude panels are all peers, and different architectures are
    /// the one free source of error independence. Mixing *levels* is the error.
    ///
    /// A uniform panel at any single level passes, including an all-small one:
    /// those voters are peers, so the lift is real, merely anchored to a lower
    /// individual accuracy.
    ///
    /// Name-based and therefore fallible — an unrecognized model is left
    /// unclassified rather than guessed at, so this warns about what it knows
    /// and never blocks.
    pub fn tier_warning(&self, default_model: &str) -> Option<String> {
        let mut by_tier: BTreeMap<Tier, Vec<String>> = BTreeMap::new();
        for p in &self.personae {
            let model = p.model.as_deref().unwrap_or(default_model);
            if let Some(tier) = tier_of(model) {
                by_tier
                    .entry(tier)
                    .or_default()
                    .push(format!("{} ({model})", p.id));
            }
        }
        if by_tier.len() < 2 {
            return None;
        }
        let levels: Vec<String> = by_tier
            .iter()
            .map(|(tier, who)| format!("  {:<9} {}", tier.name(), who.join(", ")))
            .collect();
        Some(format!(
            "this panel mixes capability levels, which breaks the majority rule \
             it relies on.\n{}\n  \
             A weaker bloc outvotes the stronger voters rather than adding to \
             them: a 5-panel at p=[.90,.60,.52,.52,.52] scores 0.71, against \
             0.90 for the strong voter alone. Fix the level and vary the \
             PROVIDER instead — claude/codex at one level are peers. Introduce \
             expertise through per-voter `context`, not through model strength.",
            levels.join("\n")
        ))
    }

    /// Boilerplate shared by most items. Nine of fourteen items opening with
    /// the same 468-character definition means most ballots carry ~470
    /// identical characters on both sides — paid for on every pair, informing
    /// none of them. It belongs in `preamble`.
    pub fn shared_prefix_warning(&self) -> Option<String> {
        let texts: Vec<Vec<char>> = self
            .items
            .iter()
            .map(|i| i.text.trim().chars().collect())
            .collect();
        let half = self.items.len().div_ceil(2);
        let longest = texts.iter().map(Vec::len).max().unwrap_or(0);
        let mut best = 0usize;
        for k in (80..=longest.min(4000)).rev() {
            let mut groups: BTreeMap<&[char], usize> = BTreeMap::new();
            for t in texts.iter().filter(|t| t.len() >= k) {
                *groups.entry(&t[..k]).or_default() += 1;
            }
            if groups.values().any(|n| *n >= half.max(2)) {
                best = k;
                break;
            }
        }
        (best >= 120).then(|| {
            format!(
                "at least half the items open with the same {best} characters. \
                 That text sits on BOTH sides of most pairs, so it cannot inform \
                 any comparison — it only dilutes the part that can. Move shared \
                 definitions into the manifest's `preamble`, which is sent once \
                 per ballot."
            )
        })
    }

    pub fn pairs_expected(&self) -> usize {
        self.items.len() * (self.items.len() - 1) / 2
    }

    pub fn votes_expected(&self) -> usize {
        self.pairs_expected() * self.personae.len()
    }
}

/// Coarse capability levels, across providers. Deliberately short and
/// deliberately incomplete: an unknown model is left unclassified, so a false
/// negative costs only a missing warning while a false positive would nag about
/// a model that is fine. Expect to edit this as families come and go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Small,
    Mid,
    Frontier,
}

impl Tier {
    fn name(self) -> &'static str {
        match self {
            Tier::Small => "small",
            Tier::Mid => "mid",
            Tier::Frontier => "frontier",
        }
    }
}

pub fn tier_of(model: &str) -> Option<Tier> {
    let m = model.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| m.contains(n));
    // Small first: "gpt-5-mini" must not match the frontier "gpt-5".
    if has(&["haiku", "mini", "nano", "flash", "lite", "small", "8b"]) {
        Some(Tier::Small)
    } else if has(&["sonnet"]) {
        Some(Tier::Mid)
    } else if has(&["opus", "fable", "gpt-5", "o3", "codex"]) {
        Some(Tier::Frontier)
    } else {
        None
    }
}

/// Catch the source document arriving through a voter's pre-context.
///
/// Items are verbatim from the source by construction, so a context file that
/// reproduces most of them *is* the source — and a voter that has read it is
/// reading the ordering, rationale and phase labels the analyst stripped. This
/// is content-based rather than path-based, so it still fires when the source
/// has been copied, renamed or excerpted.
pub fn source_leak(items: &[Item], context: &str) -> Option<String> {
    if items.len() < 3 || context.trim().is_empty() {
        return None;
    }
    let haystack = squash(context);
    let hits = items
        .iter()
        .filter(|i| {
            let needle = squash(&i.text);
            needle.len() > 12 && haystack.contains(&needle)
        })
        .count();
    (hits * 2 > items.len()).then(|| {
        format!(
            "{hits} of {} item texts appear verbatim in this voter's context — \
             it is reading the source document, including whatever ordering and \
             rationale the analyst stripped",
            items.len()
        )
    })
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn dup_check<'a>(what: &str, ids: impl Iterator<Item = &'a str>) -> R<()> {
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        if id.trim().is_empty() {
            return Err(format!("empty {what} id").into());
        }
        if !seen.insert(id) {
            return Err(format!("duplicate {what} id {id:?}").into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(items: usize, voters: usize) -> Manifest {
        Manifest {
            preamble: String::new(),
            dropped: Vec::new(),
            items: (0..items)
                .map(|n| Item {
                    id: format!("a{n}"),
                    text: format!("item {n}"),
                })
                .collect(),
            personae: (0..voters)
                .map(|n| Persona {
                    id: format!("v{n}"),
                    lens: "lens".into(),
                    context: Vec::new(),
                    bootstrap: String::new(),
                    brief: String::new(),
                    model: None,
                })
                .collect(),
        }
    }

    #[test]
    fn even_panels_are_rejected() {
        let err = manifest(4, 4).validate().unwrap_err().to_string();
        assert!(err.contains("odd"), "{err}");
        assert!(manifest(4, 5).validate().is_ok());
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let mut m = manifest(3, 3);
        m.items[1].id = "a0".into();
        assert!(m.validate().unwrap_err().to_string().contains("duplicate item"));
    }

    #[test]
    fn a_manifest_carrying_a_criterion_is_refused() {
        let dir = std::env::temp_dir().join(format!("panel-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.json");
        std::fs::write(
            &path,
            r#"{"criterion":"which is riskier","dropped":[],"items":[{"id":"a","text":"x"},
               {"id":"b","text":"y"}],"personae":[{"id":"v1","lens":"l"},
               {"id":"v2","lens":"l"},{"id":"v3","lens":"l"}]}"#,
        )
        .unwrap();
        let err = Manifest::load(path.to_str().unwrap()).unwrap_err().to_string();
        assert!(err.contains("--criterion"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn panel_on(models: [&str; 5]) -> Manifest {
        let mut m = manifest(4, 5);
        for (p, model) in m.personae.iter_mut().zip(models) {
            p.model = Some(model.into());
        }
        m
    }

    #[test]
    fn mixing_providers_at_one_level_is_fine() {
        // The whole point: architecture is the free decorrelation axis.
        for models in [
            ["opus", "gpt-5", "opus", "gpt-5", "opus"],   // claude / codex
            ["gpt-5", "gpt-5", "gpt-5", "gpt-5", "gpt-5"], // codex / codex
            ["opus", "opus", "opus", "opus", "opus"],      // claude / claude
            ["haiku", "haiku", "haiku", "haiku", "haiku"], // weak peers are peers
        ] {
            assert!(
                panel_on(models).tier_warning("opus").is_none(),
                "{models:?} should pass"
            );
        }
    }

    #[test]
    fn mixing_levels_is_flagged_whichever_levels_they_are() {
        let warning = panel_on(["opus", "claude-haiku-4-5-20251001", "opus", "opus", "opus"])
            .tier_warning("opus")
            .expect("frontier + small");
        assert!(warning.contains("mixes capability levels"));
        assert!(warning.contains("small"));
        assert!(warning.contains("frontier"));

        // Not just small-vs-rest: mid mixed with frontier is the same error.
        assert!(panel_on(["opus", "sonnet", "opus", "opus", "opus"])
            .tier_warning("opus")
            .is_some());
    }

    #[test]
    fn small_variants_are_not_mistaken_for_their_frontier_namesakes() {
        assert_eq!(tier_of("gpt-5-mini"), Some(Tier::Small));
        assert_eq!(tier_of("gpt-5"), Some(Tier::Frontier));
        assert_eq!(tier_of("claude-haiku-4-5-20251001"), Some(Tier::Small));
        assert_eq!(tier_of("some-unreleased-model"), None, "unknown stays unclassified");
    }

    #[test]
    fn an_unknown_model_never_triggers_a_level_warning() {
        assert!(panel_on(["mystery-1", "mystery-2", "opus", "opus", "opus"])
            .tier_warning("opus")
            .is_none());
    }

    #[test]
    fn the_source_document_arriving_as_pre_context_is_caught() {
        let items = vec![
            Item { id: "a".into(), text: "Rewrite the block allocator".into() },
            Item { id: "b".into(), text: "Add a write-through cache".into() },
            Item { id: "c".into(), text: "Delete the legacy shim".into() },
        ];
        let leaked = "Q3 plan, in priority order:\n1. Rewrite   the block allocator\n\
                      2. Add a write-through cache\n3. Delete the legacy shim";
        assert!(source_leak(&items, leaked).is_some(), "whitespace must not hide it");

        let innocent = "The allocator has had three incidents this year; see runbook 12.";
        assert!(source_leak(&items, innocent).is_none());
        assert!(source_leak(&items, "").is_none());
    }

    #[test]
    fn the_default_model_counts_toward_the_mix() {
        // A voter with no explicit model still has a tier.
        let mut m = manifest(4, 5);
        m.personae[0].model = Some("haiku".into());
        assert!(m.tier_warning("opus").is_some());
        assert!(m.tier_warning("haiku").is_none(), "all small once the default is small");
    }

    fn write_manifest(name: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("panel-m-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.json");
        std::fs::write(&path, body).unwrap();
        path
    }

    const VALID_REST: &str = r#""items":[{"id":"a","text":"x"},{"id":"b","text":"y"}],
        "personae":[{"id":"v1","lens":"l"},{"id":"v2","lens":"l"},{"id":"v3","lens":"l"}]"#;

    #[test]
    fn a_manifest_without_a_dropped_field_is_refused() {
        let path = write_manifest("nodrop", &format!("{{{VALID_REST}}}"));
        let err = Manifest::load(path.to_str().unwrap()).unwrap_err().to_string();
        assert!(err.contains("no `dropped` field"), "{err}");
        assert!(err.contains("Silent subsetting"));
    }

    #[test]
    fn an_empty_dropped_list_asserts_completeness_and_loads() {
        let path = write_manifest("emptydrop", &format!(r#"{{"dropped":[],{VALID_REST}}}"#));
        assert!(Manifest::load(path.to_str().unwrap()).unwrap().dropped.is_empty());
    }

    #[test]
    fn dropped_candidates_share_the_item_id_namespace() {
        let path = write_manifest(
            "dupdrop",
            &format!(r#"{{"dropped":[{{"id":"a","reason":"r"}}],{VALID_REST}}}"#),
        );
        let err = Manifest::load(path.to_str().unwrap()).unwrap_err().to_string();
        assert!(err.contains("duplicate item"), "{err}");
    }

    #[test]
    fn boilerplate_shared_by_most_items_is_flagged() {
        let boiler = "A widget is a unit of scheduled work owned by exactly one                       team and billed to exactly one cost centre, as defined in                       the platform glossary revision four. ";
        let mut m = manifest(6, 3);
        for (n, item) in m.items.iter_mut().enumerate() {
            item.text = if n < 4 {
                format!("{boiler} Distinct tail number {n}.")
            } else {
                format!("Something entirely different, number {n}.")
            };
        }
        let warning = m.shared_prefix_warning().expect("4 of 6 share a long prefix");
        assert!(warning.contains("BOTH sides"));
        assert!(warning.contains("preamble"));
    }

    #[test]
    fn genuinely_distinct_items_are_not_flagged_as_boilerplate() {
        let mut m = manifest(6, 3);
        for (n, item) in m.items.iter_mut().enumerate() {
            item.text = format!("Item {n}: a wholly separate piece of scheduled work with its own description.");
        }
        assert!(m.shared_prefix_warning().is_none());
    }

    #[test]
    fn pair_and_vote_counts_are_quadratic() {
        let m = manifest(12, 5);
        assert_eq!(m.pairs_expected(), 66);
        assert_eq!(m.votes_expected(), 330);
    }
}
