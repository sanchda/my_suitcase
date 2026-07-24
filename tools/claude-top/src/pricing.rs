//! Static per-model pricing (USD per million tokens). Substring-matched so
//! version suffixes (e.g. "claude-opus-4-8") still resolve. Update the table as
//! prices change; an unknown model degrades to `None` (never a wrong number).

struct Price { input: f64, output: f64, cache_read: f64, cache_write: f64 }

impl Price {
    /// Cache rates are fixed multiples of the input rate — 0.1x for reads and
    /// 1.25x for 5-minute writes — so only the two base rates are ever quoted.
    /// claude-top can't see a transcript's cache TTL, so it assumes 5-minute.
    const fn per_mtok(input: f64, output: f64) -> Self {
        Self { input, output, cache_read: input * 0.1, cache_write: input * 1.25 }
    }
}

/// USD per 1M tokens, matched on lowercase family substrings so version
/// suffixes still resolve. Rates as published 2026-07; an unknown model
/// degrades to `None` rather than a wrong number.
///
/// Ordering matters: legacy Opus billed at 3x the current Opus rate, so those
/// ids must be matched before the generic `opus` arm or old transcripts get
/// silently repriced.
fn price_for(model: &str) -> Option<Price> {
    let m = model.to_ascii_lowercase();
    // Opus 3 / 4.0 / 4.1 — retired or deprecated, but still in old transcripts.
    if m.contains("opus-4-1") || m.contains("opus-4-0") || m.contains("3-opus") {
        Some(Price::per_mtok(15.0, 75.0))
    }
    // Fable 5 and Mythos 5 share the top tier.
    else if m.contains("fable") || m.contains("mythos") { Some(Price::per_mtok(10.0, 50.0)) }
    else if m.contains("opus") { Some(Price::per_mtok(5.0, 25.0)) }
    else if m.contains("sonnet") { Some(Price::per_mtok(3.0, 15.0)) }
    else if m.contains("haiku") { Some(Price::per_mtok(1.0, 5.0)) }
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

    /// Cost of exactly 1M input + 1M output, i.e. `input_rate + output_rate`.
    fn per_million(model: &str) -> f64 {
        cost_usd(model, 1_000_000, 1_000_000, 0, 0).unwrap_or_else(|| panic!("no price for {model}"))
    }

    #[test]
    fn known_model_versioned_suffix_resolves() {
        // Opus 5 / 4.8: $5 in + $25 out.
        assert!((per_million("claude-opus-4-8") - 30.0).abs() < 1e-6);
    }

    #[test]
    fn prices_each_current_family_at_published_rates() {
        assert!((per_million("claude-opus-5") - 30.0).abs() < 1e-6);      // $5 + $25
        assert!((per_million("claude-fable-5") - 60.0).abs() < 1e-6);     // $10 + $50
        assert!((per_million("claude-mythos-5") - 60.0).abs() < 1e-6);    // $10 + $50
        assert!((per_million("claude-sonnet-5") - 18.0).abs() < 1e-6);    // $3 + $15
        assert!((per_million("claude-sonnet-4-6") - 18.0).abs() < 1e-6);  // $3 + $15
        assert!((per_million("claude-haiku-4-5") - 6.0).abs() < 1e-6);    // $1 + $5
    }

    #[test]
    fn legacy_opus_keeps_its_own_higher_rates() {
        // Opus 3 / 4.0 / 4.1 billed at $15 + $75 — don't reprice old transcripts
        // at the current Opus rate.
        assert!((per_million("claude-opus-4-1") - 90.0).abs() < 1e-6);
        assert!((per_million("claude-3-opus-20240229") - 90.0).abs() < 1e-6);
    }

    #[test]
    fn cache_rates_derive_from_the_input_rate() {
        // Cache reads bill at 0.1x input, 5-minute cache writes at 1.25x.
        let read = cost_usd("claude-opus-5", 0, 0, 1_000_000, 0).unwrap();
        let write = cost_usd("claude-opus-5", 0, 0, 0, 1_000_000).unwrap();
        assert!((read - 0.5).abs() < 1e-6, "got {read}");
        assert!((write - 6.25).abs() < 1e-6, "got {write}");
    }

    #[test]
    fn unknown_model_is_none() {
        assert!(cost_usd("gpt-4", 1000, 1000, 0, 0).is_none());
        // Claude Code writes this for entries with no real model behind them.
        assert!(cost_usd("<synthetic>", 1000, 1000, 0, 0).is_none());
    }
}
