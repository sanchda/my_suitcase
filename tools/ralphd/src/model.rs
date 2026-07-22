//! Validation for `/model <tier>` — only the three known tiers are accepted, and
//! the value is canonicalized (trimmed, lowercased) before it is written to
//! `.ralph/MODEL` as a one-shot override for the next iteration.

const TIERS: [&str; 3] = ["haiku", "sonnet", "opus"];

/// Return the canonical tier string if `raw` names a known tier, else `None`.
pub fn validate_tier(raw: &str) -> Option<&'static str> {
    let normalized = raw.trim().to_ascii_lowercase();
    TIERS.iter().copied().find(|t| *t == normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_known_tiers_case_insensitively() {
        assert_eq!(validate_tier("opus"), Some("opus"));
        assert_eq!(validate_tier("  Sonnet "), Some("sonnet"));
        assert_eq!(validate_tier("HAIKU"), Some("haiku"));
    }

    #[test]
    fn rejects_unknown_tiers() {
        assert_eq!(validate_tier("gpt5"), None);
        assert_eq!(validate_tier(""), None);
    }
}
