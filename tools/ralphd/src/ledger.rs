//! Per-loop spend, read from the `ledger.jsonl` that `ralph` appends one line
//! per iteration. Read-only and best-effort: enforcement lives in `ralph`, and
//! the file is absent entirely on a ralph too old to write one.

use serde::Deserialize;
use std::path::Path;

/// Summed spend over the budget window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spend {
    pub total_usd: f64,
    pub entries: usize,
}

/// The budget the loop's own `ralph.toml` declares, if any.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Budget {
    pub usd: f64,
    /// Seconds; 0 means all-time.
    pub window: u64,
}

/// Warn once spend reaches this share of the budget.
const WARN_AT: f64 = 0.8;

/// `budget_window` comes from TOML as either seconds or a suffixed string.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DurationSpec {
    Secs(u64),
    Text(String),
}

/// Only the two budget keys — ralph owns the rest of the schema, so this must
/// tolerate every other field rather than mirror it.
#[derive(Debug, Deserialize)]
struct BudgetFile {
    budget_usd: Option<f64>,
    budget_window: Option<DurationSpec>,
}

/// Seconds from `300s`/`30m`/`8h`/`1d` or a bare number, matching ralph's parser.
fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = match s.chars().last()? {
        's' => (&s[..s.len() - 1], 1),
        'm' => (&s[..s.len() - 1], 60),
        'h' => (&s[..s.len() - 1], 3600),
        'd' => (&s[..s.len() - 1], 86_400),
        c if c.is_ascii_digit() => (s, 1),
        _ => return None,
    };
    num.trim().parse::<u64>().ok().map(|n| n * mult)
}

/// Read `budget_usd`/`budget_window` from the loop's `ralph.toml`, so the
/// warning threshold is never a second copy of the number ralph enforces.
pub fn budget(ralph_config: &Path) -> Option<Budget> {
    let text = std::fs::read_to_string(ralph_config).ok()?;
    let file: BudgetFile = toml::from_str(&text).ok()?;
    let usd = file.budget_usd.filter(|v| v.is_finite() && *v > 0.0)?;
    let window = match file.budget_window {
        Some(DurationSpec::Secs(n)) => n,
        Some(DurationSpec::Text(s)) => parse_duration(&s)?,
        None => 0,
    };
    Some(Budget { usd, window })
}

/// One ledger line. Extra keys are ignored so a newer ralph can extend it.
#[derive(Debug, Deserialize)]
struct Entry {
    #[serde(default)]
    ts: u64,
    #[serde(default)]
    cost_usd: f64,
}

/// Sum `ledger.jsonl` over `window` seconds back from `now_unix` (0 = all-time).
/// Every line counts: a LIMIT or TRANSIENT retry re-appends the same `iter`, and
/// that money was really spent, so deduplicating by iteration under-reports.
pub fn spend(text: &str, window: u64, now_unix: u64) -> Spend {
    let cutoff = if window == 0 {
        0
    } else {
        now_unix.saturating_sub(window)
    };
    let mut out = Spend {
        total_usd: 0.0,
        entries: 0,
    };
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(e) = serde_json::from_str::<Entry>(line) else {
            continue;
        };
        if e.ts < cutoff || !e.cost_usd.is_finite() {
            continue;
        }
        out.total_usd += e.cost_usd;
        out.entries += 1;
    }
    out
}

/// Read and sum the loop's ledger; `None` when the file is absent or empty.
pub fn read(state_dir: &Path, window: u64, now_unix: u64) -> Option<Spend> {
    let text = std::fs::read_to_string(state_dir.join("ledger.jsonl")).ok()?;
    let s = spend(&text, window, now_unix);
    (s.entries > 0).then_some(s)
}

/// The card's spend line, or `None` when there is nothing to report.
pub fn spend_line(spend: Option<Spend>, budget: Option<Budget>) -> Option<String> {
    let s = spend?;
    let over = |b: &Budget| s.total_usd >= b.usd * WARN_AT;
    Some(match budget {
        Some(b) if over(&b) => format!(
            "⚠️ **spend ${:.2} / ${:.2}** ({:.0}% of budget) · {} iters\n",
            s.total_usd,
            b.usd,
            100.0 * s.total_usd / b.usd,
            s.entries
        ),
        Some(b) => format!(
            "-# spend ${:.2} / ${:.2} · {} iters\n",
            s.total_usd, b.usd, s.entries
        ),
        None => format!("-# spend ${:.2} · {} iters\n", s.total_usd, s.entries),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINES: &str = concat!(
        r#"{"ts":1000,"iter":1,"model":"sonnet","cost_usd":0.5}"#,
        "\n",
        r#"{"ts":2000,"iter":2,"model":"sonnet","cost_usd":0.25}"#,
        "\n",
        r#"{"ts":3000,"iter":2,"model":"opus","cost_usd":1.0}"#,
        "\n",
    );

    #[test]
    fn retried_iterations_are_summed_not_deduplicated() {
        let s = spend(LINES, 0, 4000);
        assert_eq!(s.entries, 3);
        assert!((s.total_usd - 1.75).abs() < 1e-9, "{s:?}");
    }

    #[test]
    fn window_drops_entries_older_than_the_cutoff() {
        let s = spend(LINES, 2500, 4000); // cutoff 1500
        assert_eq!(s.entries, 2);
        assert!((s.total_usd - 1.25).abs() < 1e-9, "{s:?}");
    }

    #[test]
    fn malformed_and_non_finite_lines_are_skipped() {
        let text = concat!(
            "not json\n",
            "\n",
            r#"{"ts":10,"cost_usd":null}"#,
            "\n",
            r#"{"ts":10,"iter":1,"cost_usd":0.5}"#,
            "\n",
        );
        let s = spend(text, 0, 100);
        assert_eq!(s.entries, 1);
        assert_eq!(s.total_usd, 0.5);
    }

    #[test]
    fn spend_line_warns_only_at_four_fifths_of_budget() {
        let b = Some(Budget {
            usd: 10.0,
            window: 0,
        });
        let quiet = spend_line(
            Some(Spend {
                total_usd: 7.0,
                entries: 3,
            }),
            b,
        )
        .unwrap();
        assert!(quiet.starts_with("-#"), "{quiet}");

        let loud = spend_line(
            Some(Spend {
                total_usd: 8.0,
                entries: 4,
            }),
            b,
        )
        .unwrap();
        assert!(loud.contains("⚠️"), "{loud}");
        assert!(loud.contains("80% of budget"), "{loud}");
    }

    #[test]
    fn no_ledger_means_no_line() {
        assert_eq!(spend_line(None, None), None);
    }

    #[test]
    fn budget_reads_ralph_toml_and_tolerates_its_other_keys() {
        let dir = std::env::temp_dir().join(format!("ralphd-budget-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("ralph.toml");
        std::fs::write(
            &path,
            "model = \"sonnet\"\nbudget_usd = 40.0\nbudget_window = \"24h\"\nmax_iterations = 50\n",
        )
        .unwrap();
        assert_eq!(
            budget(&path),
            Some(Budget {
                usd: 40.0,
                window: 86_400
            })
        );

        // No budget key, or no file at all → nothing to warn against.
        std::fs::write(&path, "model = \"sonnet\"\n").unwrap();
        assert_eq!(budget(&path), None);
        assert_eq!(budget(&dir.join("absent.toml")), None);
    }
}
