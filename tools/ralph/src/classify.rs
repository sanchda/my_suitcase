//! Classifying a `claude` result envelope into a control decision.
//!
//! The CLI returns exit 0 even on API errors, so the loop never trusts exit
//! codes — it parses the result envelope (`is_error`, `api_error_status`,
//! `.result` text) and maps it here. Ported faithfully from the bash
//! `classify()`: the usage/limit *text* check runs before the status-code
//! check, the transient/fatal *text* checks run after, and anything
//! unrecognized defaults to TRANSIENT (retry rather than die).

/// What the loop should do about an iteration's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Not an error — a normal completed iteration.
    Success,
    /// Usage / rate / credit limit — wait it out, unlimited retries.
    Limit,
    /// 5xx / network / crash — short backoff, unlimited retries.
    Transient,
    /// Auth / bad model / bad request — abort; looping won't fix config.
    Fatal,
}

/// Words that, appearing anywhere in the (whitespace-normalized, lowercased)
/// result text, mean a usage/rate/credit limit.
const LIMIT_WORDS: &[&str] = &[
    "usage limit",
    "credit balance",
    "out of credit",
    "quota",
    "insufficient credit",
    "insufficient quota",
    "insufficient funds",
    "reset at",
    "resets at",
    "will reset",
    "too many requests",
    "rate limit",
];

/// Words meaning a transient/retryable failure.
const TRANSIENT_WORDS: &[&str] = &[
    "overloaded",
    "internal server",
    "timeout",
    "timed out",
    "connection",
    "network",
    "econnreset",
    "socket",
    "temporarily",
];

/// Words meaning a fatal (config/auth) failure that retrying won't fix.
const FATAL_WORDS: &[&str] = &[
    "invalid api key",
    "authentication",
    "no access",
    "does not exist",
    "it may not exist",
];

/// Classify an outcome. `status` is the envelope's `api_error_status` (None when
/// absent/null). `text` is the envelope's `.result`.
pub fn classify(is_error: bool, status: Option<u16>, text: &str) -> Class {
    if !is_error {
        return Class::Success;
    }
    let t = normalize(text);
    if contains_any(&t, LIMIT_WORDS) {
        return Class::Limit;
    }
    match status {
        Some(429) => return Class::Limit,
        Some(500) | Some(502) | Some(503) | Some(504) | Some(529) => return Class::Transient,
        Some(401) | Some(403) | Some(400) | Some(404) => return Class::Fatal,
        _ => {}
    }
    if contains_any(&t, TRANSIENT_WORDS) {
        return Class::Transient;
    }
    if contains_any(&t, FATAL_WORDS) {
        return Class::Fatal;
    }
    // Default: retry rather than die.
    Class::Transient
}

/// Lowercase and collapse all whitespace runs to single spaces, so patterns
/// like "usage limit" match text with newlines/tabs between the words (the
/// bash version used `[[:space:]]+`).
fn normalize(s: &str) -> String {
    s.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_error_is_success() {
        assert_eq!(classify(false, None, "all done"), Class::Success);
        // is_error=false wins even if the text looks scary.
        assert_eq!(classify(false, Some(500), "overloaded"), Class::Success);
    }

    #[test]
    fn limit_text_beats_status() {
        // Text limit check runs before the status match.
        assert_eq!(
            classify(true, Some(500), "You have hit your usage limit; it will reset at 5pm"),
            Class::Limit
        );
    }

    #[test]
    fn limit_variants() {
        for t in [
            "usage limit reached",
            "your credit balance is too low",
            "you are out of credit",
            "monthly quota exceeded",
            "insufficient funds",
            "the limit resets at midnight",
            "your plan will reset tomorrow",
            "429 too many requests",
            "rate limit exceeded",
        ] {
            assert_eq!(classify(true, None, t), Class::Limit, "text: {t}");
        }
    }

    #[test]
    fn whitespace_between_words_still_matches() {
        assert_eq!(classify(true, None, "usage\n   limit"), Class::Limit);
        assert_eq!(classify(true, None, "TOO\tMANY\nREQUESTS"), Class::Limit);
    }

    #[test]
    fn status_codes() {
        assert_eq!(classify(true, Some(429), ""), Class::Limit);
        for s in [500, 502, 503, 504, 529] {
            assert_eq!(classify(true, Some(s), ""), Class::Transient, "status {s}");
        }
        for s in [401, 403, 400, 404] {
            assert_eq!(classify(true, Some(s), ""), Class::Fatal, "status {s}");
        }
    }

    #[test]
    fn transient_text() {
        for t in ["model overloaded", "connection reset", "network error", "timed out"] {
            assert_eq!(classify(true, None, t), Class::Transient, "text: {t}");
        }
    }

    #[test]
    fn fatal_text() {
        for t in [
            "invalid api key",
            "authentication failed",
            "you have no access to this model",
            "model does not exist",
        ] {
            assert_eq!(classify(true, None, t), Class::Fatal, "text: {t}");
        }
    }

    #[test]
    fn unknown_error_defaults_transient() {
        assert_eq!(classify(true, None, "something weird happened"), Class::Transient);
        assert_eq!(classify(true, Some(418), "teapot"), Class::Transient);
    }
}
