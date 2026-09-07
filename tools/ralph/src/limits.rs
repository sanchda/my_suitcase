//! Reset hints from CLI errors. Unrecognized or stale hints use configured backoff.
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, TimeZone, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::backend::Backend;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ProviderLimit {
    pub retry_at: i64,
    pub allow_failover: bool,
    pub backoff: u64,
}

/// Provider accounts have independent budgets, even when model tiers change.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    pub anthropic: ProviderLimit,
    pub openai: ProviderLimit,
}

impl Limits {
    pub fn get(&self, backend: Backend) -> &ProviderLimit {
        if backend == Backend::Codex {
            &self.openai
        } else {
            &self.anthropic
        }
    }

    pub fn get_mut(&mut self, backend: Backend) -> &mut ProviderLimit {
        if backend == Backend::Codex {
            &mut self.openai
        } else {
            &mut self.anthropic
        }
    }

    pub fn load(dir: &Path) -> Result<Self, String> {
        match std::fs::read(dir.join("provider-limits.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn save(&self, dir: &Path) -> crate::R<()> {
        let temp = dir.join("provider-limits.json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(temp, dir.join("provider-limits.json"))?;
        Ok(())
    }

    pub fn route(&self, primary: Backend, now: i64) -> bool {
        let limit = self.get(primary);
        limit.allow_failover && limit.retry_at > now && self.get(primary.opposite()).retry_at <= now
    }

    pub fn wait_until(&self, primary: Backend, alternate_usable: bool, now: i64) -> i64 {
        let limit = self.get(primary);
        if alternate_usable && limit.allow_failover {
            limit
                .retry_at
                .min(self.get(primary.opposite()).retry_at)
                .max(now)
        } else {
            limit.retry_at.max(now)
        }
    }
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static reset pattern")
}

/// Parse only text following a reset/retry cue, never incidental dates in prose.
/// Explicit zones win; an unzoned CLI clock is interpreted in the local zone.
pub fn reset_at(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let text = text.replace(['’', '\n', '\t'], " ");
    let cue = re(
        r"(?i)\b(?:resets?|retry[- ]after|try again|retry)(?:\s+(?:at|on|in|after))?\s*[:=]?\s*",
    );
    cue.find_iter(&text)
        .filter_map(|m| parse_hint(&text[m.end()..], now))
        .filter(|at| *at > now)
        .max()
}

fn parse_hint(hint: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    // ISO timestamps carry their own unambiguous date and offset.
    if let Some(m) =
        re(r"^\d{4}-\d{2}-\d{2}[Tt ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:[Zz]|[+-]\d{2}:\d{2})").find(hint)
    {
        return DateTime::parse_from_rfc3339(m.as_str())
            .ok()
            .map(|d| d.with_timezone(&Utc));
    }
    // Retry-After's HTTP-date form.
    if let Some(m) = re(r"(?i)^[a-z]{3}, \d{1,2} [a-z]{3} \d{4} \d{2}:\d{2}:\d{2} GMT").find(hint) {
        return DateTime::parse_from_rfc2822(m.as_str())
            .ok()
            .map(|d| d.with_timezone(&Utc));
    }
    let unit = re(
        r"(?i)^\s*(\d+(?:\.\d+)?)\s*(milliseconds?|ms|seconds?|secs?|s|minutes?|mins?|m|hours?|hrs?|h|days?|d)",
    );
    let mut rest = hint;
    let mut seconds = 0.0;
    let mut matched = false;
    while let Some(c) = unit.captures(rest) {
        if rest[c.get(0)?.end()..].starts_with(char::is_alphabetic) {
            return None;
        }
        let scale = match c[2].to_ascii_lowercase().as_str() {
            "ms" | "millisecond" | "milliseconds" => 0.001,
            s if s.starts_with('s') => 1.0,
            s if s.starts_with('m') => 60.0,
            s if s.starts_with('h') => 3600.0,
            _ => 86400.0,
        };
        seconds += c[1].parse::<f64>().ok()? * scale;
        matched = true;
        rest = rest[c.get(0)?.end()..].trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        rest = rest.strip_prefix("and ").unwrap_or(rest);
    }
    if matched {
        // A bounded conversion avoids panics on corrupt provider output.
        return (seconds.is_finite() && seconds > 0.0 && seconds <= 315_360_000.0)
            .then(|| now + Duration::seconds(seconds.ceil() as i64));
    }
    // Bare Retry-After seconds. Require a terminator so clock/date prefixes do
    // not accidentally become seconds (e.g. "5pm" or "2026-09-05").
    if let Some(c) = re(r"^(\d+)\s*(?:$|[.;])").captures(hint) {
        let seconds = c[1].parse::<i64>().ok()?;
        return (seconds > 0 && seconds <= 315_360_000).then(|| now + Duration::seconds(seconds));
    }
    parse_clock(hint, now)
}

fn parse_clock(hint: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let zone = re(r"\(([A-Za-z_]+(?:/[A-Za-z_+-]+)+|UTC|GMT)\)|\b(UTC|GMT)\b");
    let zone_capture = zone.captures(hint);
    let named_zone = zone_capture
        .as_ref()
        .map(|c| c.get(1).or_else(|| c.get(2)).unwrap().as_str());
    let tz = named_zone
        .map(str::parse::<chrono_tz::Tz>)
        .transpose()
        .ok()?;
    let today = tz
        .map(|tz| now.with_timezone(&tz).date_naive())
        .unwrap_or_else(|| now.with_timezone(&Local).date_naive());
    let ordinal = re(r"(?i)(\d)(?:st|nd|rd|th)\b");
    let hint = ordinal.replace_all(hint, "$1");
    let date_re = re(
        r"(?i)^(\d{4}-\d{2}-\d{2}|[a-z]{3,9} \d{1,2}(?:,? \d{4})?|tomorrow|today)\s*,?\s*(?:at\s+)?",
    );
    let date_match = date_re.captures(&hint);
    let mut date = today;
    let mut explicit_date = false;
    let mut no_year = false;
    let mut rest = hint.as_ref();
    if let Some(c) = date_match {
        explicit_date = true;
        let value = c[1].to_lowercase();
        date = match value.as_str() {
            "today" => today,
            "tomorrow" => today.succ_opt()?,
            _ => {
                let value = value.replace(',', "");
                no_year = !value.contains('-') && value.split_whitespace().count() == 2;
                let value = if no_year {
                    format!("{value} {}", today.year())
                } else {
                    value
                };
                ["%Y-%m-%d", "%b %d %Y", "%B %d %Y"]
                    .iter()
                    .find_map(|f| NaiveDate::parse_from_str(&value, f).ok())?
            }
        };
        rest = &hint[c.get(0)?.end()..];
    }
    let clock = re(r"(?i)^(midnight|noon|\d{1,2}(?::\d{2})?(?::\d{2})?\s*(?:am|pm)?)(?:\b|$)")
        .captures(rest)?;
    let suffix = rest[clock.get(0)?.end()..].trim_start();
    if named_zone.is_none() && (suffix.starts_with('(') || re(r"^[A-Z]{2,5}\b").is_match(suffix)) {
        // Do not silently interpret an unsupported/ambiguous zone as local.
        return None;
    }
    let value = clock[1].trim().to_lowercase();
    let time = match value.as_str() {
        "midnight" => NaiveTime::from_hms_opt(0, 0, 0)?,
        "noon" => NaiveTime::from_hms_opt(12, 0, 0)?,
        _ => {
            let mut value = value.replace(' ', "");
            if !value.contains(':') && (value.ends_with("am") || value.ends_with("pm")) {
                value.insert_str(value.len() - 2, ":00");
            }
            ["%I:%M:%S%P", "%I:%M%P", "%I%P", "%H:%M:%S", "%H:%M"]
                .iter()
                .find_map(|f| NaiveTime::parse_from_str(&value, f).ok())?
        }
    };
    let resolve = |date: NaiveDate| {
        let naive = date.and_time(time);
        // Ambiguous/nonexistent DST clocks are unsafe to guess.
        if let Some(tz) = tz {
            tz.from_local_datetime(&naive)
                .single()
                .map(|d| d.with_timezone(&Utc))
        } else {
            Local
                .from_local_datetime(&naive)
                .single()
                .map(|d| d.with_timezone(&Utc))
        }
    };
    let mut at = resolve(date)?;
    if at <= now {
        if !explicit_date {
            at = resolve(date.succ_opt()?)?;
        } else if no_year && today.month() == 12 && date.month() == 1 {
            at = resolve(date.with_year(date.year() + 1)?)?;
        }
    }
    Some(at)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn now() -> DateTime<Utc> {
        "2026-09-05T20:00:00Z".parse().unwrap()
    }
    #[test]
    fn reset_output_formats() {
        for (text, expected) in [
            (
                "rate limit; try again in 2 hours, 15 minutes and 3 seconds",
                "2026-09-05T22:15:03Z",
            ),
            ("try again in 2h10m", "2026-09-05T22:10:00Z"),
            ("retry after 250ms", "2026-09-05T20:00:01Z"),
            ("Retry-After: 60", "2026-09-05T20:01:00Z"),
            (
                "Retry-After: Sat, 05 Sep 2026 22:00:00 GMT",
                "2026-09-05T22:00:00Z",
            ),
            (
                "Usage limit resets at 2026-09-05T17:00:00-05:00",
                "2026-09-05T22:00:00Z",
            ),
            (
                "You've hit your limit · resets 5pm (America/Chicago)",
                "2026-09-05T22:00:00Z",
            ),
            ("resets at 1pm (America/Chicago)", "2026-09-06T18:00:00Z"),
            ("resets at midnight (UTC)", "2026-09-06T00:00:00Z"),
            (
                "try again at Sep 5th, 2026 5:30 PM (America/Chicago).",
                "2026-09-05T22:30:00Z",
            ),
            (
                "resets Sep 12 at 5pm (America/Chicago)",
                "2026-09-12T22:00:00Z",
            ),
            ("will reset tomorrow at noon UTC", "2026-09-06T12:00:00Z"),
        ] {
            assert_eq!(
                reset_at(text, now()),
                Some(expected.parse().unwrap()),
                "{text}"
            );
        }
    }
    #[test]
    fn unknown_invalid_or_past_hints_use_fallback() {
        for text in [
            "quota exceeded",
            "finished at 5pm",
            "resets eventually",
            "resets at 2025-01-01T00:00:00Z",
            "resets at 99pm",
            "retry in 999999999999999999999 hours",
            "retry in 0 seconds",
            "resets 5pm (Invalid/Zone)",
            "resets 5pm (PST)",
            "resets 5pm CST",
            "resets Sep 5 at 1pm (America/Chicago)",
            "resets Mar 8, 2026 at 2:30am (America/Chicago)",
        ] {
            assert_eq!(reset_at(text, now()), None, "{text}");
        }
    }
    #[test]
    fn dst_ambiguity_falls_back_and_year_boundary_is_resolved() {
        let now = "2026-11-01T00:00:00Z".parse().unwrap();
        assert_eq!(
            reset_at("resets Nov 1, 2026 at 1:30am (America/Chicago)", now),
            None
        );
        let now = "2026-12-31T23:00:00Z".parse().unwrap();
        assert_eq!(
            reset_at("resets Jan 1 at noon UTC", now),
            Some("2027-01-01T12:00:00Z".parse().unwrap())
        );
    }

    #[test]
    fn independent_deadlines_and_earliest_usable_provider() {
        let mut limits = Limits {
            anthropic: ProviderLimit {
                retry_at: 100,
                allow_failover: true,
                backoff: 30,
            },
            ..Limits::default()
        };
        assert!(limits.route(Backend::Claude, 10));
        limits.openai = ProviderLimit {
            retry_at: 40,
            allow_failover: true,
            backoff: 15,
        };
        assert!(!limits.route(Backend::Claude, 10));
        assert_eq!(limits.wait_until(Backend::Claude, true, 10), 40);
        assert_eq!(limits.wait_until(Backend::Claude, false, 10), 100);
        assert!(limits.route(Backend::Claude, 40));
        assert!(!limits.route(Backend::Claude, 100));
        *limits.get_mut(Backend::Codex) = ProviderLimit::default();
        assert_eq!(limits.anthropic.retry_at, 100);
        assert_eq!(limits.anthropic.backoff, 30);
        let restored: Limits =
            serde_json::from_str(&serde_json::to_string(&limits).unwrap()).unwrap();
        assert_eq!(restored.anthropic.retry_at, 100);
    }
}
