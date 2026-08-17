//! `<dir>/ledger.jsonl` — one append-only line per iteration, so a spend budget
//! survives the restarts an in-memory `cost_total` does not.

use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const FILE: &str = "ledger.jsonl";

/// One iteration's spend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub ts: u64,
    pub iter: u64,
    pub model: String,
    pub cost_usd: f64,
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Append one iteration's spend. Single writer, so `O_APPEND` needs no lock.
pub fn append(dir: &Path, iter: u64, model: &str, cost_usd: f64) -> crate::R<()> {
    let entry = Entry {
        ts: now(),
        iter,
        model: model.to_string(),
        // A non-finite envelope cost serializes to `null` and makes the line unparseable.
        cost_usd: if cost_usd.is_finite() { cost_usd } else { 0.0 },
    };
    let line = serde_json::to_string(&entry)?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path(dir))?;
    writeln!(f, "{line}")?;
    Ok(())
}

/// Total recorded spend within the last `window_secs` (0 = the whole ledger).
/// A missing ledger, an unparseable line, or a non-finite cost contributes 0 —
/// a corrupt tail must never fabricate a budget halt.
pub fn spend_since(dir: &Path, window_secs: u64) -> f64 {
    let text = match std::fs::read_to_string(path(dir)) {
        Ok(t) => t,
        Err(_) => return 0.0,
    };
    let cutoff = if window_secs == 0 {
        0
    } else {
        now().saturating_sub(window_secs)
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<Entry>(l).ok())
        .filter(|e| e.ts >= cutoff && e.cost_usd.is_finite())
        .map(|e| e.cost_usd)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmp() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ralph-ledger-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn appends_accumulate() {
        let dir = tmp();
        assert_eq!(spend_since(&dir, 0), 0.0); // absent ledger
        append(&dir, 1, "sonnet", 0.42).unwrap();
        append(&dir, 2, "opus", 1.5).unwrap();
        assert!((spend_since(&dir, 0) - 1.92).abs() < 1e-9);
        assert_eq!(
            std::fs::read_to_string(path(&dir)).unwrap().lines().count(),
            2
        );
    }

    #[test]
    fn line_shape_matches_the_documented_schema() {
        let dir = tmp();
        append(&dir, 7, "sonnet", 0.42).unwrap();
        let raw = std::fs::read_to_string(path(&dir)).unwrap();
        let line = raw.lines().next().unwrap();
        assert!(line.starts_with(r#"{"ts":"#), "{line}");
        assert!(line.contains(r#""iter":7"#));
        assert!(line.contains(r#""model":"sonnet""#));
        assert!(line.contains(r#""cost_usd":0.42"#));
        serde_json::from_str::<Entry>(line).unwrap();
    }

    #[test]
    fn non_finite_cost_is_written_as_zero() {
        let dir = tmp();
        append(&dir, 1, "sonnet", f64::NAN).unwrap();
        append(&dir, 2, "sonnet", f64::INFINITY).unwrap();
        append(&dir, 3, "sonnet", f64::NEG_INFINITY).unwrap();
        let raw = std::fs::read_to_string(path(&dir)).unwrap();
        for line in raw.lines() {
            assert!(!line.contains("null"), "unparseable line: {line}");
            assert_eq!(serde_json::from_str::<Entry>(line).unwrap().cost_usd, 0.0);
        }
        assert_eq!(spend_since(&dir, 0), 0.0);
    }

    #[test]
    fn window_excludes_older_entries() {
        let dir = tmp();
        let old = Entry {
            ts: now() - 7200,
            iter: 1,
            model: "sonnet".into(),
            cost_usd: 10.0,
        };
        let recent = Entry {
            ts: now() - 60,
            iter: 2,
            model: "sonnet".into(),
            cost_usd: 2.0,
        };
        let body = format!(
            "{}\n{}\n",
            serde_json::to_string(&old).unwrap(),
            serde_json::to_string(&recent).unwrap()
        );
        std::fs::write(path(&dir), body).unwrap();
        assert_eq!(spend_since(&dir, 3600), 2.0);
        assert_eq!(spend_since(&dir, 0), 12.0); // unbounded window
    }

    #[test]
    fn corrupt_lines_are_skipped() {
        let dir = tmp();
        append(&dir, 1, "sonnet", 1.0).unwrap();
        let mut f = OpenOptions::new().append(true).open(path(&dir)).unwrap();
        writeln!(f, "{{not json").unwrap();
        writeln!(f, r#"{{"ts":1,"iter":2,"model":"opus","cost_usd":null}}"#).unwrap();
        drop(f);
        append(&dir, 3, "opus", 2.0).unwrap();
        assert_eq!(spend_since(&dir, 0), 3.0);
    }
}
