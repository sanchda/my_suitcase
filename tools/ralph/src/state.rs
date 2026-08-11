//! Reading and writing the gitignored `.ralph/` runtime directory: the
//! iteration counter, the agent's `MODEL`/`STATUS` hand-offs, the live status
//! file, the raw per-iteration logs, and `run.log`.

use crate::R;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Handle on a `.ralph/` runtime directory.
pub struct State {
    pub dir: PathBuf,
}

/// The parsed consolidated end-of-turn report from `.ralph/HANDOFF.json`.
/// Every field is optional and pre-validated; `None` means "not declared".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Handoff {
    /// Iteration type: `code`, `plan`, `review`, or `blocked`.
    pub status: Option<String>,
    /// One-shot model override for the NEXT iteration (validated tier).
    pub model: Option<String>,
    /// Human-readable dead-end reason accompanying `status: blocked`.
    pub blocked: Option<String>,
}

impl State {
    /// Open (creating `dir` and `dir/logs`) a runtime directory.
    pub fn open(dir: &Path) -> R<State> {
        fs::create_dir_all(dir.join("logs"))?;
        Ok(State {
            dir: dir.to_path_buf(),
        })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Current iteration counter (0 if unset).
    pub fn iteration(&self) -> u64 {
        fs::read_to_string(self.path("iteration"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Persist the iteration counter.
    pub fn set_iteration(&self, n: u64) -> R<()> {
        fs::write(self.path("iteration"), format!("{n}\n"))?;
        Ok(())
    }

    /// The agent's requested next model tier, validated against `allowed`.
    /// Returns `None` (and logs a warning) for absent/empty/invalid values so a
    /// typo never aborts the loop.
    pub fn read_model(&self, allowed: &[String]) -> Option<String> {
        let raw = fs::read_to_string(self.path("MODEL")).ok()?;
        let m: String = raw.split_whitespace().collect();
        if m.is_empty() {
            return None;
        }
        if allowed.iter().any(|a| a == &m) {
            Some(m)
        } else {
            self.log(&format!("  ⚠ ignoring invalid .ralph/MODEL ('{m}')"));
            None
        }
    }

    /// Read `.ralph/MODEL` and clear it. The agent's directive is a one-shot
    /// override for the *next* pass only, so a stale value never sticks and the
    /// resolved leaf's own `(tier/…)` decoration governs by default.
    pub fn take_model(&self, allowed: &[String]) -> Option<String> {
        let picked = self.read_model(allowed);
        let _ = fs::remove_file(self.path("MODEL"));
        picked
    }

    /// Write the one-shot MODEL override (used to bridge a HANDOFF-declared
    /// model to the next iteration's existing `take_model` path).
    pub fn write_model(&self, model: &str) {
        let _ = fs::write(self.path("MODEL"), format!("{model}\n"));
    }

    /// Read and remove `.ralph/HANDOFF.json` — the consolidated end-of-turn
    /// report (`{"status": "...", "model": "...", "blocked": "..."}`). The
    /// legacy STATUS/MODEL files remain honored when no handoff is present.
    /// Malformed JSON or invalid field values warn and are ignored — a bad
    /// handoff never aborts the loop.
    pub fn take_handoff(&self, allowed_models: &[String]) -> Option<Handoff> {
        let path = self.path("HANDOFF.json");
        let raw = fs::read_to_string(&path).ok()?;
        let _ = fs::remove_file(&path);
        let value: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                self.log(&format!("  ⚠ ignoring malformed .ralph/HANDOFF.json: {e}"));
                return None;
            }
        };
        let field = |name: &str| {
            value
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let status = field("status").map(str::to_lowercase).filter(|s| {
            let ok = matches!(s.as_str(), "code" | "plan" | "review" | "blocked");
            if !ok {
                self.log(&format!("  ⚠ ignoring invalid HANDOFF status ('{s}')"));
            }
            ok
        });
        let model = field("model").map(str::to_string).filter(|m| {
            let ok = allowed_models.iter().any(|a| a == m);
            if !ok {
                self.log(&format!("  ⚠ ignoring invalid HANDOFF model ('{m}')"));
            }
            ok
        });
        let blocked = field("blocked").map(str::to_string);
        Some(Handoff {
            status,
            model,
            blocked,
        })
    }

    /// The agent's declared iteration type from `.ralph/STATUS` (lowercased,
    /// trimmed). `None` if absent/empty — the caller treats that as `code`.
    pub fn read_status(&self) -> Option<String> {
        let raw = fs::read_to_string(self.path("STATUS")).ok()?;
        let s = raw.trim().to_lowercase();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    /// Clear the per-iteration `STATUS` descriptor after reading it, so an
    /// iteration that doesn't declare a type defaults to `code` rather than
    /// inheriting the previous iteration's value. (`MODEL` is cleared the same
    /// way at read time by [`Self::take_model`] — it directs only the next pass.)
    pub fn clear_status(&self) {
        let _ = fs::remove_file(self.path("STATUS"));
    }

    /// Overwrite the live status file (`.ralph/live`) shown to the operator.
    ///
    /// Deliberately **not** named `status`: the agent's hand-off file is
    /// `STATUS`, and on case-insensitive filesystems (macOS) `status` and
    /// `STATUS` are the same file — a live write would then be misread as the
    /// agent's iteration type.
    pub fn write_live_status(&self, text: &str) {
        let _ = fs::write(self.path("live"), text);
    }

    /// Persist the last result envelope JSON.
    pub fn write_last_result(&self, json: &str) {
        let _ = fs::write(self.path("last-result.json"), json);
    }

    /// Path for iteration `n`'s raw log, and (re)point `current.log` at it.
    pub fn new_iter_log(&self, n: u64) -> R<PathBuf> {
        let name = format!("logs/iter-{:04}-{}.log", n, timestamp());
        let full = self.dir.join(&name);
        fs::File::create(&full)?;
        // Best-effort: ignore symlink failures.
        let link = self.path("current.log");
        let _ = fs::remove_file(&link);
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(&name, &link);
        Ok(full)
    }

    /// STOP file present? (graceful-halt request)
    pub fn stop_requested(&self) -> bool {
        self.path("STOP").exists()
    }

    /// Remove the STOP file after honoring it.
    pub fn clear_stop(&self) {
        let _ = fs::remove_file(self.path("STOP"));
    }

    /// Write the STOP marker (used by `ralph stop`). The running loop halts
    /// gracefully after the current task, and the supervisor suppresses restart.
    pub fn request_stop(&self) -> R<()> {
        fs::write(self.path("STOP"), "stop requested\n")?;
        Ok(())
    }

    /// Write the START marker (used by `ralph start`): a request for a watching
    /// ralphd to launch the loop. Symmetric to STOP; ralphd consumes it. Lets a
    /// separate local process (e.g. a claude session) trigger the ralphd-managed
    /// loop without going through Discord.
    pub fn request_start(&self) -> R<()> {
        fs::write(self.path("START"), "start requested\n")?;
        Ok(())
    }

    pub fn baseline_path(&self) -> PathBuf {
        self.path("git-baseline")
    }

    /// Append a timestamped line to `run.log` and echo it to stdout.
    pub fn log(&self, msg: &str) {
        let line = format!("{} {}\n", clock(), msg);
        print!("{line}");
        let _ = std::io::stdout().flush();
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path("run.log"))
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// `HH:MM:SS` UTC time-of-day (matches the bash `date -u +%H:%M:%S`).
fn clock() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let tod = secs % 86_400;
    format!("{:02}:{:02}:{:02}", tod / 3600, (tod % 3600) / 60, tod % 60)
}

/// `YYYYMMDDTHHMMSSZ` UTC stamp for log filenames.
pub(crate) fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d) = civil_from_days((secs / 86_400) as i64);
    let tod = secs % 86_400;
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        y,
        mo,
        d,
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

/// Convert days-since-epoch to a civil (year, month, day) — Howard Hinnant's
/// algorithm. Keeps log filenames dated without pulling in a date crate.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unique temp dir per call — process id alone is shared across threads, so
    // the parallel test runner would otherwise race two tests onto one path.
    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "ralph-state-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&base);
        base
    }

    #[test]
    fn iteration_roundtrip() {
        let s = State::open(&tmp()).unwrap();
        assert_eq!(s.iteration(), 0);
        s.set_iteration(7).unwrap();
        assert_eq!(s.iteration(), 7);
    }

    #[test]
    fn model_validation() {
        let s = State::open(&tmp()).unwrap();
        let allowed: Vec<String> = ["haiku", "sonnet", "opus"]
            .iter()
            .map(|x| x.to_string())
            .collect();
        assert_eq!(s.read_model(&allowed), None); // absent
        fs::write(s.path("MODEL"), "  opus \n").unwrap();
        assert_eq!(s.read_model(&allowed), Some("opus".into()));
        fs::write(s.path("MODEL"), "gpt5").unwrap();
        assert_eq!(s.read_model(&allowed), None); // invalid ignored
    }

    #[test]
    fn status_reading() {
        let s = State::open(&tmp()).unwrap();
        assert_eq!(s.read_status(), None);
        fs::write(s.path("STATUS"), "Review\n").unwrap();
        assert_eq!(s.read_status(), Some("review".into()));
        fs::write(s.path("STATUS"), "   ").unwrap();
        assert_eq!(s.read_status(), None);
    }

    #[test]
    fn handoff_roundtrip_and_validation() {
        let s = State::open(&tmp()).unwrap();
        let allowed: Vec<String> = ["haiku", "sonnet", "opus"]
            .iter()
            .map(|x| x.to_string())
            .collect();
        assert_eq!(s.take_handoff(&allowed), None); // absent

        // Valid handoff: parsed once, then the file is consumed.
        fs::write(
            s.path("HANDOFF.json"),
            r#"{"status": "Blocked", "model": "opus", "blocked": "needs an API key"}"#,
        )
        .unwrap();
        let h = s.take_handoff(&allowed).unwrap();
        assert_eq!(h.status.as_deref(), Some("blocked"));
        assert_eq!(h.model.as_deref(), Some("opus"));
        assert_eq!(h.blocked.as_deref(), Some("needs an API key"));
        assert_eq!(s.take_handoff(&allowed), None); // consumed

        // Invalid field values are dropped individually, not fatally.
        fs::write(
            s.path("HANDOFF.json"),
            r#"{"status": "heroic", "model": "gpt5"}"#,
        )
        .unwrap();
        let h = s.take_handoff(&allowed).unwrap();
        assert_eq!(h.status, None);
        assert_eq!(h.model, None);

        // Malformed JSON is ignored entirely (legacy files then govern).
        fs::write(s.path("HANDOFF.json"), "{not json").unwrap();
        assert_eq!(s.take_handoff(&allowed), None);
        assert!(!s.path("HANDOFF.json").exists(), "consumed even when bad");
    }

    #[test]
    fn write_model_feeds_take_model() {
        let s = State::open(&tmp()).unwrap();
        let allowed: Vec<String> = vec!["sonnet".into(), "opus".into()];
        s.write_model("opus");
        assert_eq!(s.take_model(&allowed), Some("opus".into()));
        assert_eq!(s.take_model(&allowed), None); // one-shot
    }

    #[test]
    fn request_stop_writes_marker() {
        let s = State::open(&tmp()).unwrap();
        assert!(!s.stop_requested());
        s.request_stop().unwrap();
        assert!(s.stop_requested());
        s.clear_stop();
        assert!(!s.stop_requested());
    }

    #[test]
    fn request_start_writes_marker() {
        let dir = tmp();
        let s = State::open(&dir).unwrap();
        assert!(!dir.join("START").exists());
        s.request_start().unwrap();
        assert!(dir.join("START").exists());
    }

    #[test]
    fn civil_date_known_values() {
        // 2026-07-17 is day 20651 since 1970-01-01.
        assert_eq!(civil_from_days(20_651), (2026, 7, 17));
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }
}
