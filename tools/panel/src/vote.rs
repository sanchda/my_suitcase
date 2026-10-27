//! Ballot parsing and the on-disk vote record.
//!
//! Parsing is deliberately lenient about formatting (models add bullets, bold
//! and stray prose) and strict about meaning: a line whose choice is not clearly
//! A or B is dropped with a warning rather than guessed at. A missing vote is
//! visible in the tally's coverage check; an invented one is not.

use serde::{Deserialize, Serialize};

pub const REASON_CHAR_CAP: usize = 200;

/// One vote, as written to `votes.jsonl`. The first four fields are the schema
/// the Python tally used, so old vote files still read.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Vote {
    pub pair: [String; 2],
    pub voter: String,
    pub choice: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// Kept because cycles are findings, and the reasons are what make a
    /// reported cycle mean anything.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// "ab" when this voter saw `pair[0]` first. Lets a run be audited for
    /// position bias after the fact.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub orientation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Side {
    A,
    B,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ballot {
    pub index: usize,
    pub side: Side,
    pub confidence: Option<f64>,
    pub reason: String,
}

/// Parse a voter's reply into at most one ballot per `expected` pair.
/// Returns `(ballots, warnings)`; `ballots[i]` is `None` when pair `i` went
/// unanswered or unparseable.
pub fn parse_ballots(raw: &str, expected: usize) -> (Vec<Option<Ballot>>, Vec<String>) {
    let mut out: Vec<Option<Ballot>> = vec![None; expected];
    let mut warnings = Vec::new();

    for line in raw.lines() {
        let line = line.trim().trim_start_matches(['-', '*', '•']).trim();
        let line = line.trim_matches('`').trim();
        if line.is_empty() || !line.contains('|') {
            continue;
        }
        let fields: Vec<&str> = line.splitn(4, '|').map(str::trim).collect();
        if fields.len() < 2 {
            continue;
        }
        let index = match parse_index(fields[0]) {
            Some(n) if n >= 1 && n <= expected => n - 1,
            Some(n) => {
                warnings.push(format!("pair number {n} out of range 1..={expected}"));
                continue;
            }
            None => continue,
        };
        let side = match parse_side(fields[1]) {
            Some(side) => side,
            None => {
                warnings.push(format!("pair {}: choice {:?} is not A or B", index + 1, fields[1]));
                continue;
            }
        };
        let confidence = fields.get(2).and_then(|f| parse_confidence(f));
        let reason = fields.get(3).map(|r| cap(r)).unwrap_or_default();
        if out[index].is_some() {
            warnings.push(format!("pair {} answered more than once; kept the first", index + 1));
            continue;
        }
        out[index] = Some(Ballot {
            index,
            side,
            confidence,
            reason,
        });
    }

    let missing = out.iter().filter(|b| b.is_none()).count();
    if missing > 0 {
        warnings.push(format!("{missing} of {expected} pairs unanswered"));
    }
    (out, warnings)
}

fn parse_index(field: &str) -> Option<usize> {
    let digits: String = field
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn parse_side(field: &str) -> Option<Side> {
    let cleaned: String = field
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_uppercase();
    match cleaned.as_str() {
        "A" => Some(Side::A),
        "B" => Some(Side::B),
        _ => None,
    }
}

fn parse_confidence(field: &str) -> Option<f64> {
    let percent = field.contains('%');
    let numeric: String = field
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let value: f64 = numeric.parse().ok()?;
    let value = if percent { value / 100.0 } else { value };
    (0.0..=1.0).contains(&value).then_some(value)
}

fn cap(reason: &str) -> String {
    let reason = reason.trim();
    if reason.chars().count() <= REASON_CHAR_CAP {
        return reason.to_string();
    }
    reason.chars().take(REASON_CHAR_CAP).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_format() {
        let (ballots, warnings) = parse_ballots(
            "1|A|0.80|A fails loudly, B fails silently.\n2|B|0.55|B is cheaper to undo.",
            2,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        let first = ballots[0].clone().unwrap();
        assert_eq!(first.side, Side::A);
        assert_eq!(first.confidence, Some(0.8));
        assert_eq!(first.reason, "A fails loudly, B fails silently.");
        assert_eq!(ballots[1].clone().unwrap().side, Side::B);
    }

    #[test]
    fn tolerates_the_decorations_models_actually_add() {
        let (ballots, _) = parse_ballots(
            "Here are my judgments:\n\n- `1 | **A** | 80% | cheaper`\n* 2|b|0.4|later\n\nDone.",
            2,
        );
        assert_eq!(ballots[0].clone().unwrap().side, Side::A);
        assert_eq!(ballots[0].clone().unwrap().confidence, Some(0.8));
        assert_eq!(ballots[1].clone().unwrap().side, Side::B);
    }

    #[test]
    fn an_unreadable_choice_is_dropped_not_guessed() {
        let (ballots, warnings) = parse_ballots("1|both|0.5|cannot decide\n2|A|0.9|clear", 2);
        assert!(ballots[0].is_none());
        assert!(warnings.iter().any(|w| w.contains("not A or B")));
        assert!(ballots[1].is_some());
    }

    #[test]
    fn missing_answers_are_reported_against_the_expected_count() {
        let (ballots, warnings) = parse_ballots("1|A|0.9|only one", 3);
        assert_eq!(ballots.iter().filter(|b| b.is_some()).count(), 1);
        assert!(warnings.iter().any(|w| w.contains("2 of 3 pairs unanswered")));
    }

    #[test]
    fn out_of_range_and_repeat_answers_warn() {
        let (_, warnings) = parse_ballots("9|A|0.5|x\n1|A|0.5|first\n1|B|0.5|second", 2);
        assert!(warnings.iter().any(|w| w.contains("out of range")));
        assert!(warnings.iter().any(|w| w.contains("more than once")));
    }

    #[test]
    fn nonsense_confidence_degrades_to_none_without_losing_the_vote() {
        let (ballots, _) = parse_ballots("1|A|high|still a vote", 1);
        let b = ballots[0].clone().unwrap();
        assert_eq!(b.confidence, None);
        assert_eq!(b.side, Side::A);
    }

    #[test]
    fn reasons_are_capped() {
        let long = "x".repeat(500);
        let (ballots, _) = parse_ballots(&format!("1|A|0.5|{long}"), 1);
        assert_eq!(ballots[0].clone().unwrap().reason.chars().count(), REASON_CHAR_CAP);
    }
}
