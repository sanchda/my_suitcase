//! `ralph model <tier>` — write the one-shot `.ralph/MODEL` override that
//! `State::take_model` consumes on the next iteration. Lives here, not in
//! ralphd, so the file format has exactly one owner.

use crate::state::State;
use crate::R;

/// The ladder entry `raw` names (trimmed, case-insensitive), else `None`.
/// Returns the ladder's own spelling because `State::read_model` matches it
/// exactly.
pub fn validate_tier<'a>(raw: &str, ladder: &'a [String]) -> Option<&'a str> {
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }
    ladder
        .iter()
        .find(|t| t.eq_ignore_ascii_case(&normalized))
        .map(String::as_str)
}

/// `ralph model <tier>`. Exit 0 on success, 1 on an unknown tier.
pub fn run(args: &[String]) -> R<i32> {
    let tier = args
        .first()
        .filter(|a| !a.starts_with('-'))
        .ok_or("usage: ralph model <tier>")?;
    let rest = args.get(1..).unwrap_or(&[]);
    let cfg = crate::config::load_base(rest)?;
    match validate_tier(tier, &cfg.escalation_ladder) {
        Some(canonical) => {
            State::open(&cfg.dir)?.write_model(canonical);
            println!("ralph: next iteration will run `{canonical}` (one-shot override)");
            Ok(0)
        }
        None => {
            eprintln!(
                "ralph: unknown model tier '{tier}' — expected one of: {}",
                cfg.escalation_ladder.join(", ")
            );
            Ok(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ladder() -> Vec<String> {
        vec!["haiku".into(), "sonnet".into(), "opus".into()]
    }

    #[test]
    fn accepts_known_tiers_case_insensitively() {
        assert_eq!(validate_tier("opus", &ladder()), Some("opus"));
        assert_eq!(validate_tier("  Sonnet ", &ladder()), Some("sonnet"));
        assert_eq!(validate_tier("HAIKU", &ladder()), Some("haiku"));
    }

    #[test]
    fn rejects_unknown_tiers() {
        assert_eq!(validate_tier("gpt5", &ladder()), None);
        assert_eq!(validate_tier("", &ladder()), None);
        assert_eq!(validate_tier("   ", &ladder()), None);
    }

    #[test]
    fn a_configured_ladder_governs() {
        let custom = vec!["sonnet".to_string()];
        assert_eq!(validate_tier("sonnet", &custom), Some("sonnet"));
        assert_eq!(validate_tier("opus", &custom), None);
    }

    #[test]
    fn written_override_is_what_take_model_accepts() {
        let dir = std::env::temp_dir().join(format!("ralph-model-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = State::open(&dir).unwrap();
        state.write_model(validate_tier(" Opus ", &ladder()).unwrap());
        assert_eq!(state.take_model(&ladder()), Some("opus".into()));
        assert_eq!(state.take_model(&ladder()), None); // one-shot
    }
}
