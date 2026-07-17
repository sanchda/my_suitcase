//! Static per-model pricing (USD per million tokens). Substring-matched so
//! version suffixes (e.g. "claude-opus-4-8") still resolve. Update the table as
//! prices change; an unknown model degrades to `None` (never a wrong number).

struct Price { input: f64, output: f64, cache_read: f64, cache_write: f64 }

/// USD per 1M tokens. Keep keys as lowercase family substrings.
fn price_for(model: &str) -> Option<Price> {
    let m = model.to_ascii_lowercase();
    // NOTE: illustrative rates; adjust to current published pricing.
    if m.contains("opus") { Some(Price { input: 15.0, output: 75.0, cache_read: 1.5, cache_write: 18.75 }) }
    else if m.contains("sonnet") { Some(Price { input: 3.0, output: 15.0, cache_read: 0.3, cache_write: 3.75 }) }
    else if m.contains("haiku") { Some(Price { input: 0.8, output: 4.0, cache_read: 0.08, cache_write: 1.0 }) }
    else if m.contains("fable") { Some(Price { input: 15.0, output: 75.0, cache_read: 1.5, cache_write: 18.75 }) }
    else { None }
}

/// Estimated cost in USD, or `None` if the model is not in the table.
pub fn cost_usd(model: &str, input: u64, output: u64, cache_read: u64, cache_write: u64) -> Option<f64> {
    let p = price_for(model)?;
    let per = |toks: u64, rate: f64| (toks as f64) / 1_000_000.0 * rate;
    Some(per(input, p.input) + per(output, p.output) + per(cache_read, p.cache_read) + per(cache_write, p.cache_write))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_model_versioned_suffix_resolves() {
        // 1M input at $15 + 1M output at $75 = $90.00
        let c = cost_usd("claude-opus-4-8", 1_000_000, 1_000_000, 0, 0).unwrap();
        assert!((c - 90.0).abs() < 1e-6, "got {c}");
    }

    #[test]
    fn unknown_model_is_none() {
        assert!(cost_usd("gpt-4", 1000, 1000, 0, 0).is_none());
    }
}
