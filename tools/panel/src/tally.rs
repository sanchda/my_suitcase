//! Aggregation: per-pair majority, then Copeland over the majority graph.
//!
//! Deterministic by construction — there is no model judgment in this file.
//! Discretion at the last step would undo the discipline of every step before
//! it, which is why aggregation is a program and not a read of the tally by
//! something with an opinion.

use crate::vote::Vote;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize)]
pub struct PairOutcome {
    pub pair: [String; 2],
    pub winner: Option<String>,
    pub margin: [u32; 2],
    /// Why each voter chose what it chose. A reported cycle is only meaningful
    /// alongside these.
    pub reasons: Vec<[String; 3]>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Rank {
    pub rank: usize,
    pub id: String,
    pub copeland: u32,
    pub losses: u32,
    pub votes_received: u32,
}

/// Per-voter diagnostics. `concordance` is **leave-one-out**: how often this
/// voter agreed with the majority of the OTHER voters. Including the voter's
/// own ballot would bias it upward, which is exactly the direction that would
/// hide the problem.
#[derive(Debug, Clone, Serialize)]
pub struct VoterStat {
    pub id: String,
    pub votes: usize,
    pub concordance: Option<f64>,
    /// Share of this voter's reasons admitting its evidence did not cover the
    /// pair. High rates mean the lens aligns with the item set rather than
    /// cutting across it — see `degenerate_parliament`.
    pub silence_rate: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub items: usize,
    pub pairs_expected: usize,
    pub pairs_found: usize,
    pub pairs_decided: usize,
    pub panel_size: usize,
    pub unanimity_rate: Option<f64>,
    pub ranking: Vec<Rank>,
    pub pairs: Vec<PairOutcome>,
    pub cycles: Vec<Vec<String>>,
    pub voters: Vec<VoterStat>,
    pub warnings: Vec<String>,
}

/// `known_items` is the manifest's item list when available. Without it an item
/// that drew no votes at all is invisible: the expected pair count would be
/// derived from the items that happen to appear, so the gap could never be
/// detected. With it, a silently dropped item is caught.
pub fn tally(votes: &[Vote], known_items: Option<&[String]>) -> Report {
    let mut warnings = Vec::new();
    let mut buckets: BTreeMap<(String, String), BTreeMap<String, (String, String)>> =
        BTreeMap::new();

    for vote in votes {
        let (x, y) = (vote.pair[0].clone(), vote.pair[1].clone());
        if x == y {
            warnings.push(format!("{x} paired with itself; vote discarded"));
            continue;
        }
        let key = if x < y { (x, y) } else { (y, x) };
        if vote.choice != key.0 && vote.choice != key.1 {
            warnings.push(format!(
                "choice {:?} is not in pair {} vs {}; vote discarded",
                vote.choice, key.0, key.1
            ));
            continue;
        }
        let by_voter = buckets.entry(key.clone()).or_default();
        if by_voter.contains_key(&vote.voter) {
            // Last-write-wins would silently pick one of two contradictory
            // votes. Keep the first and say so.
            warnings.push(format!(
                "{} voted twice on {} vs {}; kept the first",
                vote.voter, key.0, key.1
            ));
            continue;
        }
        by_voter.insert(
            vote.voter.clone(),
            (vote.choice.clone(), vote.reason.clone()),
        );
    }

    let mut outcomes: Vec<PairOutcome> = Vec::new();
    let mut wins: BTreeMap<String, u32> = BTreeMap::new();
    let mut losses: BTreeMap<String, u32> = BTreeMap::new();
    let mut received: BTreeMap<String, u32> = BTreeMap::new();
    let mut edges: Vec<(String, String)> = Vec::new();

    for ((a, b), by_voter) in &buckets {
        let (mut ca, mut cb) = (0u32, 0u32);
        for (choice, _) in by_voter.values() {
            if choice == a {
                ca += 1;
            } else {
                cb += 1;
            }
        }
        *received.entry(a.clone()).or_default() += ca;
        *received.entry(b.clone()).or_default() += cb;
        let reasons = by_voter
            .iter()
            .map(|(voter, (choice, reason))| {
                [voter.clone(), choice.clone(), reason.clone()]
            })
            .collect();
        let winner = match ca.cmp(&cb) {
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(a.clone()),
            std::cmp::Ordering::Less => Some(b.clone()),
        };
        if let Some(w) = &winner {
            let loser = if w == a { b } else { a };
            *wins.entry(w.clone()).or_default() += 1;
            *losses.entry(loser.clone()).or_default() += 1;
            edges.push((w.clone(), loser.clone()));
        }
        outcomes.push(PairOutcome {
            pair: [a.clone(), b.clone()],
            winner,
            margin: [ca.max(cb), ca.min(cb)],
            reasons,
        });
    }

    let mut items: BTreeSet<String> = buckets
        .keys()
        .flat_map(|(a, b)| [a.clone(), b.clone()])
        .collect();
    if let Some(known) = known_items {
        for id in known {
            if !items.contains(id) {
                warnings.push(format!("item {id} drew no votes at all"));
            }
            items.insert(id.clone());
        }
        for id in &items {
            if !known.contains(id) {
                warnings.push(format!("item {id} was voted on but is not in the manifest"));
            }
        }
    }
    let items: Vec<String> = items.into_iter().collect();
    let pairs_expected = items.len() * items.len().saturating_sub(1) / 2;

    // Distinct voters, not vote rows: a duplicate must not inflate panel size.
    let panel_size = buckets.values().map(|bv| bv.len()).max().unwrap_or(0);
    let decided = outcomes.iter().filter(|o| o.winner.is_some()).count();
    // Unanimous means the whole panel agreed — a pair only two voters answered
    // is not unanimous just because those two matched.
    let unanimous = outcomes
        .iter()
        .filter(|o| {
            o.winner.is_some()
                && o.margin[1] == 0
                && (o.margin[0] as usize) == panel_size
        })
        .count();

    match outcomes.len().cmp(&pairs_expected) {
        std::cmp::Ordering::Less => warnings.push(format!(
            "{} pair(s) missing of {pairs_expected}",
            pairs_expected - outcomes.len()
        )),
        std::cmp::Ordering::Greater => warnings.push(format!(
            "{} more pair(s) than the {pairs_expected} the item set allows",
            outcomes.len() - pairs_expected
        )),
        std::cmp::Ordering::Equal => {}
    }

    let cycles = strongly_connected(&items, &edges);
    let voters = concordance(&buckets);

    let mut ranking: Vec<String> = items.clone();
    ranking.sort_by(|x, y| {
        let (wx, wy) = (wins.get(x).copied().unwrap_or(0), wins.get(y).copied().unwrap_or(0));
        let (rx, ry) = (
            received.get(x).copied().unwrap_or(0),
            received.get(y).copied().unwrap_or(0),
        );
        wy.cmp(&wx).then(ry.cmp(&rx)).then(x.cmp(y))
    });

    Report {
        items: items.len(),
        pairs_expected,
        pairs_found: outcomes.len(),
        pairs_decided: decided,
        panel_size,
        unanimity_rate: (decided > 0).then(|| unanimous as f64 / decided as f64),
        ranking: ranking
            .iter()
            .enumerate()
            .map(|(n, id)| Rank {
                rank: n + 1,
                id: id.clone(),
                copeland: wins.get(id).copied().unwrap_or(0),
                losses: losses.get(id).copied().unwrap_or(0),
                votes_received: received.get(id).copied().unwrap_or(0),
            })
            .collect(),
        pairs: outcomes,
        cycles,
        voters,
        warnings,
    }
}

/// Leave-one-out concordance per voter: over the pairs it answered, how often
/// did it agree with the majority of the *other* voters?
///
/// This is the empirical parity check, and it is why it exists. The Condorcet
/// lift assumes voters of comparable accuracy; a weak bloc with a simple
/// majority rule does not merely add noise, it can outvote the strong voters
/// and land the panel below its own best member. Neither extreme is healthy:
///
/// - near 1.0 — the voter is redundant, contributing no independent evidence
/// - near 0.5 — a coin flip, or genuinely orthogonal; both need a human look
///
/// Ties among the other voters are skipped, since they name no majority to
/// agree or disagree with.
type Buckets = BTreeMap<(String, String), BTreeMap<String, (String, String)>>;

fn concordance(buckets: &Buckets) -> Vec<VoterStat> {
    let mut ids: BTreeSet<&str> = BTreeSet::new();
    for by_voter in buckets.values() {
        ids.extend(by_voter.keys().map(String::as_str));
    }

    ids.into_iter()
        .map(|id| {
            let (mut agreed, mut comparable, mut votes) = (0usize, 0usize, 0usize);
            for ((a, b), by_voter) in buckets {
                let Some((mine, _)) = by_voter.get(id) else {
                    continue;
                };
                votes += 1;
                let (mut ca, mut cb) = (0usize, 0usize);
                for (voter, (choice, _)) in by_voter {
                    if voter == id {
                        continue;
                    }
                    if choice == a {
                        ca += 1;
                    } else if choice == b {
                        cb += 1;
                    }
                }
                let others = match ca.cmp(&cb) {
                    std::cmp::Ordering::Equal => continue,
                    std::cmp::Ordering::Greater => a,
                    std::cmp::Ordering::Less => b,
                };
                comparable += 1;
                if mine == others {
                    agreed += 1;
                }
            }
            let silent = buckets
                .values()
                .filter_map(|bv| bv.get(id))
                .filter(|(_, reason)| reads_as_silence(reason))
                .count();
            VoterStat {
                id: id.to_string(),
                votes,
                concordance: (comparable > 0).then(|| agreed as f64 / comparable as f64),
                silence_rate: if votes > 0 {
                    silent as f64 / votes as f64
                } else {
                    0.0
                },
            }
        })
        .collect()
}

/// Does this reason admit the voter's evidence did not reach the pair?
fn reads_as_silence(reason: &str) -> bool {
    let r = reason.to_lowercase();
    [
        "silent on",
        "no evidence",
        "nothing in my",
        "not covered",
        "doesn\u{27}t cover",
        "does not cover",
        "outside my",
        "beyond my",
        "no information",
        "cannot assess",
        "can\u{27}t assess",
        "not addressed",
        "my evidence does not",
        "my evidence doesn\u{27}t",
    ]
    .iter()
    .any(|marker| r.contains(marker))
}

/// Kosaraju. A component of size > 1 is an intransitive cycle in the majority
/// graph — a finding to report, not a defect to smooth over.
fn strongly_connected(nodes: &[String], edges: &[(String, String)]) -> Vec<Vec<String>> {
    let mut fwd: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut rev: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (src, dst) in edges {
        fwd.entry(src).or_default().push(dst);
        rev.entry(dst).or_default().push(src);
    }

    let mut order: Vec<&str> = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for root in nodes {
        if seen.contains(root.as_str()) {
            continue;
        }
        let mut stack = vec![(root.as_str(), false)];
        while let Some((node, done)) = stack.pop() {
            if done {
                order.push(node);
                continue;
            }
            if seen.contains(node) {
                continue;
            }
            seen.insert(node);
            stack.push((node, true));
            for next in fwd.get(node).into_iter().flatten() {
                if !seen.contains(next) {
                    stack.push((next, false));
                }
            }
        }
    }

    let mut components = Vec::new();
    let mut assigned: BTreeSet<&str> = BTreeSet::new();
    for node in order.iter().rev() {
        if assigned.contains(node) {
            continue;
        }
        let mut component = Vec::new();
        let mut stack = vec![*node];
        assigned.insert(node);
        while let Some(current) = stack.pop() {
            component.push(current.to_string());
            for prev in rev.get(current).into_iter().flatten() {
                if assigned.insert(prev) {
                    stack.push(prev);
                }
            }
        }
        if component.len() > 1 {
            component.sort();
            components.push(component);
        }
    }
    components
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vote(a: &str, b: &str, voter: &str, choice: &str) -> Vote {
        Vote {
            pair: [a.into(), b.into()],
            voter: voter.into(),
            choice: choice.into(),
            confidence: Some(0.8),
            reason: "because".into(),
            orientation: "ab".into(),
        }
    }

    /// a>b, b>c, c>a, everything beats d.
    fn cyclic_panel() -> Vec<Vote> {
        let beats = [("a", "b", "a"), ("b", "c", "b"), ("a", "c", "c"),
                     ("a", "d", "a"), ("b", "d", "b"), ("c", "d", "c")];
        let mut votes = Vec::new();
        for (x, y, w) in beats {
            for v in 0..5 {
                votes.push(vote(x, y, &format!("v{v}"), w));
            }
        }
        votes
    }

    #[test]
    fn finds_the_condorcet_cycle_and_ranks_the_dominated_item_last() {
        let report = tally(&cyclic_panel(), None);
        assert_eq!(report.cycles, vec![vec!["a".to_string(), "b".into(), "c".into()]]);
        assert_eq!(report.ranking.last().unwrap().id, "d");
        assert_eq!(report.ranking.last().unwrap().copeland, 0);
        assert_eq!(report.unanimity_rate, Some(1.0));
        assert_eq!(report.panel_size, 5);
    }

    #[test]
    fn orientation_does_not_split_a_pair_in_two() {
        // Counterbalancing means half the panel reports the pair reversed.
        let votes = vec![
            vote("a", "b", "v1", "a"),
            vote("b", "a", "v2", "a"),
            vote("b", "a", "v3", "b"),
        ];
        let report = tally(&votes, None);
        assert_eq!(report.pairs_found, 1);
        assert_eq!(report.pairs[0].winner, Some("a".into()));
        assert_eq!(report.pairs[0].margin, [2, 1]);
    }

    #[test]
    fn a_self_pair_is_discarded_rather_than_becoming_a_phantom_tie() {
        let mut votes = cyclic_panel();
        votes.push(vote("a", "a", "v1", "a"));
        let report = tally(&votes, None);
        assert_eq!(report.pairs_found, 6);
        assert!(report.warnings.iter().any(|w| w.contains("paired with itself")));
    }

    #[test]
    fn a_duplicate_vote_keeps_the_first_and_does_not_inflate_panel_size() {
        let votes = vec![
            vote("a", "b", "v1", "a"),
            vote("a", "b", "v1", "b"), // contradicts itself
            vote("a", "b", "v2", "a"),
        ];
        let report = tally(&votes, None);
        assert_eq!(report.panel_size, 2, "duplicate must not count as a third voter");
        assert_eq!(report.pairs[0].margin, [2, 0], "the first vote stands");
        assert!(report.warnings.iter().any(|w| w.contains("voted twice")));
    }

    #[test]
    fn an_item_that_drew_no_votes_is_caught_only_against_the_manifest() {
        let votes = cyclic_panel();
        let blind = tally(&votes, None);
        assert!(!blind.warnings.iter().any(|w| w.contains("no votes at all")));

        let known: Vec<String> = ["a", "b", "c", "d", "e"].iter().map(|s| s.to_string()).collect();
        let report = tally(&votes, Some(&known));
        assert!(report.warnings.iter().any(|w| w.contains("item e drew no votes")));
        assert!(report.warnings.iter().any(|w| w.contains("4 pair(s) missing of 10")));
    }

    #[test]
    fn a_tie_yields_no_edge_and_is_excluded_from_unanimity() {
        let votes = vec![
            vote("a", "b", "v1", "a"),
            vote("a", "b", "v2", "b"),
            vote("a", "c", "v1", "a"),
            vote("a", "c", "v2", "a"),
        ];
        let report = tally(&votes, None);
        assert_eq!(report.pairs_found, 2);
        assert_eq!(report.pairs_decided, 1);
        assert_eq!(report.unanimity_rate, Some(1.0));
        assert!(report.pairs.iter().any(|p| p.winner.is_none()));
    }

    #[test]
    fn a_partly_answered_pair_is_not_counted_unanimous() {
        let votes = vec![
            vote("a", "b", "v1", "a"),
            vote("a", "b", "v2", "a"),
            vote("a", "b", "v3", "a"),
            vote("a", "c", "v1", "a"), // only one of three voters answered
        ];
        let report = tally(&votes, None);
        assert_eq!(report.panel_size, 3);
        assert_eq!(report.unanimity_rate, Some(0.5), "1 of 2 decided pairs is truly unanimous");
    }

    #[test]
    fn a_choice_outside_its_pair_is_discarded() {
        let report = tally(&[vote("a", "b", "v1", "zzz")], None);
        assert_eq!(report.pairs_found, 0);
        assert!(report.warnings.iter().any(|w| w.contains("is not in pair")));
    }

    #[test]
    fn reasons_survive_aggregation() {
        let report = tally(&[vote("a", "b", "v1", "a")], None);
        assert_eq!(report.pairs[0].reasons[0], ["v1".to_string(), "a".into(), "because".into()]);
    }

    #[test]
    fn concordance_is_leave_one_out_so_a_contrarian_cannot_inflate_itself() {
        // Four voters always pick the first item; v5 always picks the second.
        let mut votes = Vec::new();
        for (x, y) in [("a", "b"), ("a", "c"), ("b", "c"), ("a", "d")] {
            for v in 1..=4 {
                votes.push(vote(x, y, &format!("v{v}"), x));
            }
            votes.push(vote(x, y, "v5", y));
        }
        let report = tally(&votes, None);
        let stat = |id: &str| {
            report.voters.iter().find(|v| v.id == id).unwrap().concordance.unwrap()
        };
        assert_eq!(stat("v5"), 0.0, "contrarian must not count its own ballot");
        assert_eq!(stat("v1"), 1.0);
        assert_eq!(report.voters.iter().find(|v| v.id == "v5").unwrap().votes, 4);
    }

    #[test]
    fn a_unanimous_panel_scores_every_voter_fully_concordant() {
        // The signal for "you are paying N x for one opinion" shows up per voter.
        let report = tally(&cyclic_panel(), None);
        assert_eq!(report.voters.len(), 5);
        assert!(report.voters.iter().all(|v| v.concordance == Some(1.0)));
    }

    #[test]
    fn concordance_is_none_when_the_others_never_reach_a_majority() {
        // Leaving v1 out leaves v2 and v3 split, so there is no majority for
        // v1 to have agreed or disagreed with. v3 still scores: v1 and v2 agree.
        let votes = vec![
            vote("a", "b", "v1", "a"),
            vote("a", "b", "v2", "a"),
            vote("a", "b", "v3", "b"),
        ];
        let report = tally(&votes, None);
        let stat = |id: &str| report.voters.iter().find(|v| v.id == id).unwrap().concordance;
        assert_eq!(stat("v1"), None, "v2 and v3 split, so there is no majority");
        assert_eq!(stat("v2"), None);
        assert_eq!(stat("v3"), Some(0.0), "v1 and v2 agreed, v3 did not");
    }

    #[test]
    fn evidence_silence_is_counted_per_voter() {
        let mut votes = Vec::new();
        for (x, y) in [("a", "b"), ("a", "c"), ("b", "c")] {
            // v1 admits the gap; v2 and v3 answer from their evidence.
            let mut silent = vote(x, y, "v1", x);
            silent.reason = "My evidence is silent on the second item.".into();
            votes.push(silent);
            for v in ["v2", "v3"] {
                votes.push(vote(x, y, v, x));
            }
        }
        let report = tally(&votes, None);
        let stat = |id: &str| report.voters.iter().find(|v| v.id == id).unwrap().silence_rate;
        assert_eq!(stat("v1"), 1.0);
        assert_eq!(stat("v2"), 0.0);
    }

    #[test]
    fn silence_detection_does_not_fire_on_an_ordinary_reason() {
        let mut v = vote("a", "b", "v1", "a");
        v.reason = "A fails loudly in CI; B fails silently in production.".into();
        // "silently" must not match the "silent on" marker.
        assert_eq!(tally(&[v], None).voters[0].silence_rate, 0.0);
    }

    #[test]
    fn copeland_ties_break_on_total_votes_received() {
        // a and b each win one pair, but a wins its pair 3-0 and b wins 2-1.
        let votes = vec![
            vote("a", "c", "v1", "a"), vote("a", "c", "v2", "a"), vote("a", "c", "v3", "a"),
            vote("b", "c", "v1", "b"), vote("b", "c", "v2", "b"), vote("b", "c", "v3", "c"),
        ];
        let report = tally(&votes, None);
        assert_eq!(report.ranking[0].id, "a");
        assert_eq!(report.ranking[1].id, "b");
    }
}
