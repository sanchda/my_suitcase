//! `ralph model <name>` — write the one-shot `.ralph/MODEL` override that
//! `State::take_model` consumes on the next iteration.

use crate::state::State;
use crate::R;

/// The ladder entry `raw` names (trimmed, case-insensitive), else `None`.
/// Returns the ladder's canonical spelling.
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

/// Preserve canonical tier spelling, while allowing concrete CLI model IDs.
/// The backend CLI checks model availability.
pub fn normalize_model(raw: &str, ladder: &[String]) -> Option<String> {
    let raw = raw.trim();
    if let Some(model) = raw.strip_prefix('!') {
        if !crate::backend::valid_model(raw) {
            return None;
        }
        return normalize_model(model, ladder).map(|m| format!("!{m}"));
    }
    if let Some(tier) = validate_tier(raw, ladder) {
        return Some(tier.into());
    }
    let lower = raw.to_ascii_lowercase();
    if crate::backend::is_tier(&lower)
        || matches!(lower.as_str(), "fable" | "astra" | "sol" | "terra" | "luna")
    {
        return Some(lower);
    }
    crate::backend::valid_model(raw).then(|| raw.into())
}

/// `ralph model <name>`. Exit 0 on success, 1 on a malformed model identifier.
pub fn run(args: &[String]) -> R<i32> {
    const USAGE: &str = "usage: ralph model <name> [--dir <path>] [--config <file>]";
    if args
        .first()
        .is_some_and(|a| matches!(a.as_str(), "--help" | "-h"))
    {
        println!("{USAGE}");
        return Ok(0);
    }
    let tier = args.first().filter(|a| !a.starts_with('-')).ok_or(USAGE)?;
    let rest = args.get(1..).unwrap_or(&[]);
    let mut cfg = crate::config::load_base(rest)?;
    let mut flags = rest.iter();
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--dir" | "--config" => {
                let value = flags
                    .next()
                    .ok_or_else(|| format!("{flag} needs a value"))?;
                if flag == "--dir" {
                    cfg.dir = value.into();
                }
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(0);
            }
            _ => return Err(format!("unknown arg: {flag}").into()),
        }
    }
    match normalize_model(tier, &cfg.escalation_ladder) {
        Some(canonical) => {
            // Surface write failures to the CLI rather than reporting a lost override.
            State::open(&cfg.dir)?;
            std::fs::write(cfg.dir.join("MODEL"), format!("{canonical}\n"))?;
            println!("ralph: next iteration will run `{canonical}` (one-shot override)");
            Ok(0)
        }
        None => {
            eprintln!(
                "ralph: invalid model name '{tier}' — expected a tier or a single model identifier"
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

    #[test]
    fn exclusive_overrides_preserve_the_marker_and_normalize_aliases() {
        assert_eq!(
            normalize_model(" !ASTRA ", &ladder()).as_deref(),
            Some("!astra")
        );
        assert_eq!(
            normalize_model("!Fable", &ladder()).as_deref(),
            Some("!fable")
        );
        for bad in ["!", "!!astra", "!-model", "!fable,sonnet"] {
            assert!(normalize_model(bad, &ladder()).is_none(), "{bad}");
        }
    }
}
