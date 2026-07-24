//! Native parsing/aggregation of Claude Code transcript JSONL. The transcript
//! format is internal and may shift, so we read tolerantly via serde_json::Value
//! and skip anything that doesn't look like an assistant usage record.

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Toks {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Toks {
    pub fn add(&mut self, o: &Toks) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
    }
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

#[derive(Debug, Clone)]
pub struct Record {
    pub model: String,
    pub date: NaiveDate,
    pub toks: Toks,
    /// Working directory Claude Code stamped on the entry. Used to attribute a
    /// transcript to a running instance when its session id isn't readable from
    /// the process tree.
    pub cwd: Option<String>,
    /// Entry time, kept alongside `date` so recency comparisons don't have to
    /// round to a whole day.
    pub at: DateTime<Utc>,
}

/// Parse a single JSONL line into a usage Record, or None if it isn't one.
pub fn parse_line(line: &str) -> Option<Record> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let msg = v.get("message")?;
    let model = msg.get("model")?.as_str()?.to_string();
    let usage = msg.get("usage")?;
    let g = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
    let toks = Toks {
        input: g("input_tokens"),
        output: g("output_tokens"),
        cache_read: g("cache_read_input_tokens"),
        cache_write: g("cache_creation_input_tokens"),
    };
    let ts = v.get("timestamp")?.as_str()?;
    let parsed = DateTime::parse_from_rfc3339(ts).ok()?;
    let date = parsed.with_timezone(&chrono::Local).date_naive();
    let cwd = v.get("cwd").and_then(Value::as_str).map(|s| s.to_string());
    Some(Record { model, date, toks, cwd, at: parsed.with_timezone(&Utc) })
}

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window { Today, Week, All }

#[derive(Debug, Clone)]
pub struct ModelUsage { pub model: String, pub toks: Toks, pub cost_usd: Option<f64> }

#[derive(Debug, Clone)]
pub struct InstanceUsage { pub session_id: String, pub model: Option<String>, pub tokens: u64, pub cost_usd: Option<f64> }

#[derive(Debug, Clone)]
pub struct UsageSnapshot { pub by_model: Vec<ModelUsage>, pub by_instance: Vec<InstanceUsage> }

/// Per-file accumulation. One transcript file == one session.
#[derive(Default)]
struct FileAgg {
    offset: u64,
    session_id: String,
    model_daily: HashMap<(String, NaiveDate), Toks>,
    session_toks: Toks,
    latest_model: Option<String>,
    /// Most recent `cwd` seen in this transcript, so a session that changed
    /// directory is matched on where it actually is now.
    latest_cwd: Option<String>,
    /// Timestamp of the newest usage entry seen in this transcript.
    last_at: Option<DateTime<Utc>>,
}

#[derive(Default)]
pub struct Collector {
    files: HashMap<PathBuf, FileAgg>,
}

impl Collector {
    pub fn new() -> Self { Self::default() }

    /// Read only newly-appended bytes of one transcript file into its aggregate.
    /// If the file shrank (rotation/rewrite), reset and re-read from the start.
    pub fn refresh_file(&mut self, path: &Path) {
        let len = match std::fs::metadata(path) { Ok(m) => m.len(), Err(_) => return };
        let session_id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let agg = self.files.entry(path.to_path_buf()).or_default();
        if agg.session_id.is_empty() { agg.session_id = session_id; }
        if len < agg.offset {
            *agg = FileAgg { session_id: agg.session_id.clone(), ..Default::default() };
        }
        if len == agg.offset { return; }
        let mut f = match std::fs::File::open(path) { Ok(f) => f, Err(_) => return };
        if f.seek(SeekFrom::Start(agg.offset)).is_err() { return; }
        let reader = BufReader::new(&mut f);
        for line in reader.lines().map_while(Result::ok) {
            if let Some(r) = parse_line(&line) {
                agg.model_daily.entry((r.model.clone(), r.date)).or_default().add(&r.toks);
                agg.session_toks.add(&r.toks);
                agg.latest_model = Some(r.model);
                // Entries are appended in time order, but don't rely on it:
                // only advance cwd when this entry is genuinely the newest.
                if agg.last_at.is_none_or(|prev| r.at >= prev) {
                    agg.last_at = Some(r.at);
                    if r.cwd.is_some() { agg.latest_cwd = r.cwd; }
                }
            }
        }
        agg.offset = len;
    }

    /// Refresh every `<config_dir>/projects/**/*.jsonl` file.
    pub fn refresh_dir(&mut self, config_dir: &Path) {
        let root = config_dir.join("projects");
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) { Ok(e) => e, Err(_) => continue };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() { stack.push(p); }
                else if p.extension().and_then(|s| s.to_str()) == Some("jsonl") { self.refresh_file(&p); }
            }
        }
    }

    /// Session ids whose transcript's latest `cwd` is `cwd` and which have been
    /// active at or after `active_since`, most recently active first.
    ///
    /// This is the fallback path for identifying a running instance: the exact
    /// route (`CLAUDE_CODE_SESSION_ID` from the process tree) only works while
    /// the instance happens to have a live child process, so an idle instance
    /// is matched on its working directory instead. `active_since` keeps a
    /// freshly-launched instance from inheriting an old session's totals.
    pub fn sessions_for_cwd(&self, cwd: &Path, active_since: DateTime<Utc>) -> Vec<String> {
        let mut hits: Vec<(DateTime<Utc>, &str)> = self.files.values()
            .filter(|a| a.latest_cwd.as_deref() == cwd.to_str())
            .filter_map(|a| a.last_at.filter(|at| *at >= active_since).map(|at| (at, a.session_id.as_str())))
            .collect();
        hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        hits.into_iter().map(|(_, sid)| sid.to_string()).collect()
    }

    /// Build the two panels. `today` is passed in so callers/tests are deterministic.
    /// `running` scopes `by_instance` to sessions that are currently running
    /// (matched by session id); `by_model` remains the account-wide windowed
    /// aggregate over all transcript files regardless of what's running.
    pub fn snapshot(&self, window: Window, today: NaiveDate, running: &std::collections::HashSet<String>) -> UsageSnapshot {
        let in_window = |d: NaiveDate| match window {
            Window::All => true,
            Window::Today => d == today,
            Window::Week => (today - d).num_days() < 7 && d <= today,
        };
        let mut by_model_map: HashMap<String, Toks> = HashMap::new();
        for agg in self.files.values() {
            for ((model, date), toks) in &agg.model_daily {
                if in_window(*date) { by_model_map.entry(model.clone()).or_default().add(toks); }
            }
        }
        let mut by_model: Vec<ModelUsage> = by_model_map.into_iter().map(|(model, toks)| {
            let cost_usd = cost_of(&model, &toks);
            ModelUsage { model, toks, cost_usd }
        }).collect();
        by_model.sort_by(|a, b| b.toks.total().cmp(&a.toks.total()).then_with(|| a.model.cmp(&b.model)));

        let mut by_instance: Vec<InstanceUsage> = self.files.values()
            .filter(|a| a.session_toks.total() > 0 && running.contains(&a.session_id))
            .map(|a| {
                let model = a.latest_model.clone();
                let cost_usd = model.as_deref().map(|m| cost_of(m, &a.session_toks)).unwrap_or(None);
                InstanceUsage { session_id: a.session_id.clone(), model, tokens: a.session_toks.total(), cost_usd }
            }).collect();
        by_instance.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.session_id.cmp(&b.session_id)));

        UsageSnapshot { by_model, by_instance }
    }
}

/// Single adapter so pricing takes a Toks without coupling the pricing module
/// to it; used by both the by_model and by_instance paths.
fn cost_of(model: &str, t: &Toks) -> Option<f64> {
    crate::pricing::cost_usd(model, t.input, t.output, t.cache_read, t.cache_write)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_assistant_usage_line() {
        let line = r#"{"type":"assistant","timestamp":"2026-07-17T12:00:00.000Z","message":{"model":"claude-opus-4-8","usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":5,"cache_creation_input_tokens":3}}}"#;
        let r = parse_line(line).unwrap();
        assert_eq!(r.model, "claude-opus-4-8");
        assert_eq!(r.toks, Toks { input: 10, output: 20, cache_read: 5, cache_write: 3 });
    }

    #[test]
    fn skips_non_usage_lines() {
        assert!(parse_line(r#"{"type":"user","message":{"role":"user"}}"#).is_none());
        assert!(parse_line("not json").is_none());
        assert!(parse_line("").is_none());
    }

    use std::io::Write;

    fn line(model: &str, day: &str, inp: u64, out: u64) -> String {
        format!(r#"{{"timestamp":"{day}T09:00:00.000Z","message":{{"model":"{model}","usage":{{"input_tokens":{inp},"output_tokens":{out},"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#)
    }

    #[test]
    fn aggregates_by_model_with_window() {
        let dir = std::env::temp_dir().join(format!("ctop-usage-{}", std::process::id()));
        let proj = dir.join("projects/p");
        std::fs::create_dir_all(&proj).unwrap();
        let f = proj.join("sess-A.jsonl");
        let mut fh = std::fs::File::create(&f).unwrap();
        writeln!(fh, "{}", line("claude-opus-4-8", "2026-07-17", 100, 10)).unwrap();
        writeln!(fh, "{}", line("claude-opus-4-8", "2026-07-01", 5, 5)).unwrap();
        drop(fh);

        let mut c = Collector::new();
        c.refresh_dir(&dir);
        let today = NaiveDate::from_ymd_opt(2026, 7, 17).unwrap();
        let running: std::collections::HashSet<String> = ["sess-A".to_string()].into_iter().collect();

        let today_snap = c.snapshot(Window::Today, today, &running);
        assert_eq!(today_snap.by_model.len(), 1);
        assert_eq!(today_snap.by_model[0].toks.input, 100);

        let all_snap = c.snapshot(Window::All, today, &running);
        assert_eq!(all_snap.by_model[0].toks.input, 105);

        assert_eq!(all_snap.by_instance.len(), 1);
        assert_eq!(all_snap.by_instance[0].session_id, "sess-A");
        assert_eq!(all_snap.by_instance[0].tokens, 105 + 10 + 5); // input+output across both lines
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn incremental_append_counts_once() {
        let dir = std::env::temp_dir().join(format!("ctop-inc-{}", std::process::id()));
        let proj = dir.join("projects/p");
        std::fs::create_dir_all(&proj).unwrap();
        let f = proj.join("sess-B.jsonl");
        let mut fh = std::fs::File::create(&f).unwrap();
        writeln!(fh, "{}", line("claude-sonnet-5", "2026-07-17", 100, 0)).unwrap();
        drop(fh);

        let mut c = Collector::new();
        c.refresh_dir(&dir);
        let today = NaiveDate::from_ymd_opt(2026, 7, 17).unwrap();
        let running = std::collections::HashSet::new();
        assert_eq!(c.snapshot(Window::All, today, &running).by_model[0].toks.input, 100);

        let mut fh = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
        writeln!(fh, "{}", line("claude-sonnet-5", "2026-07-17", 50, 0)).unwrap();
        drop(fh);
        c.refresh_dir(&dir); // must add only the new 50, not re-add the first 100
        assert_eq!(c.snapshot(Window::All, today, &running).by_model[0].toks.input, 150);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A usage line carrying the `cwd` field that Claude Code stamps on every
    /// transcript entry, at a specific wall-clock time.
    fn line_at(model: &str, ts: &str, cwd: &str, inp: u64) -> String {
        format!(
            r#"{{"timestamp":"{ts}","cwd":"{cwd}","message":{{"model":"{model}","usage":{{"input_tokens":{inp},"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
        )
    }

    fn write_lines(path: &Path, lines: &[String]) {
        let mut fh = std::fs::File::create(path).unwrap();
        for l in lines { writeln!(fh, "{l}").unwrap(); }
    }

    fn utc(s: &str) -> DateTime<chrono::Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&chrono::Utc)
    }

    #[test]
    fn sessions_for_cwd_returns_matching_sessions_most_recent_first() {
        let dir = std::env::temp_dir().join(format!("ctop-cwd-{}", std::process::id()));
        let proj = dir.join("projects/p");
        std::fs::create_dir_all(&proj).unwrap();

        write_lines(&proj.join("sess-old.jsonl"), &[line_at("claude-opus-4-8", "2026-07-17T09:00:00Z", "/w/proj", 10)]);
        write_lines(&proj.join("sess-new.jsonl"), &[line_at("claude-opus-4-8", "2026-07-17T11:00:00Z", "/w/proj", 20)]);
        write_lines(&proj.join("sess-other.jsonl"), &[line_at("claude-opus-4-8", "2026-07-17T12:00:00Z", "/w/elsewhere", 30)]);

        let mut c = Collector::new();
        c.refresh_dir(&dir);

        let since = utc("2026-07-17T00:00:00Z");
        assert_eq!(
            c.sessions_for_cwd(Path::new("/w/proj"), since),
            vec!["sess-new".to_string(), "sess-old".to_string()]
        );
        assert!(c.sessions_for_cwd(Path::new("/w/unknown"), since).is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sessions_for_cwd_excludes_sessions_idle_since_before_cutoff() {
        let dir = std::env::temp_dir().join(format!("ctop-stale-{}", std::process::id()));
        let proj = dir.join("projects/p");
        std::fs::create_dir_all(&proj).unwrap();

        write_lines(&proj.join("sess-stale.jsonl"), &[line_at("claude-opus-4-8", "2026-07-10T09:00:00Z", "/w/proj", 10)]);
        write_lines(&proj.join("sess-live.jsonl"), &[line_at("claude-opus-4-8", "2026-07-17T09:00:00Z", "/w/proj", 20)]);

        let mut c = Collector::new();
        c.refresh_dir(&dir);

        // Cutoff sits between the two: only the recent session is a candidate,
        // so a freshly-launched instance can't inherit last week's tokens.
        let since = utc("2026-07-16T00:00:00Z");
        assert_eq!(c.sessions_for_cwd(Path::new("/w/proj"), since), vec!["sess-live".to_string()]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sessions_for_cwd_tracks_latest_cwd_after_a_directory_change() {
        let dir = std::env::temp_dir().join(format!("ctop-chdir-{}", std::process::id()));
        let proj = dir.join("projects/p");
        std::fs::create_dir_all(&proj).unwrap();

        write_lines(&proj.join("sess-moved.jsonl"), &[
            line_at("claude-opus-4-8", "2026-07-17T09:00:00Z", "/w/before", 10),
            line_at("claude-opus-4-8", "2026-07-17T10:00:00Z", "/w/after", 10),
        ]);

        let mut c = Collector::new();
        c.refresh_dir(&dir);
        let since = utc("2026-07-17T00:00:00Z");

        assert_eq!(c.sessions_for_cwd(Path::new("/w/after"), since), vec!["sess-moved".to_string()]);
        assert!(c.sessions_for_cwd(Path::new("/w/before"), since).is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn by_instance_scoped_to_running() {
        let dir = std::env::temp_dir().join(format!("ctop-scoped-{}", std::process::id()));
        let proj = dir.join("projects/p");
        std::fs::create_dir_all(&proj).unwrap();

        let fa = proj.join("sess-A.jsonl");
        let mut fh = std::fs::File::create(&fa).unwrap();
        writeln!(fh, "{}", line("claude-opus-4-8", "2026-07-17", 100, 10)).unwrap();
        drop(fh);

        let fb = proj.join("sess-B.jsonl");
        let mut fh = std::fs::File::create(&fb).unwrap();
        writeln!(fh, "{}", line("claude-opus-4-8", "2026-07-17", 40, 4)).unwrap();
        drop(fh);

        let mut c = Collector::new();
        c.refresh_dir(&dir);
        let today = NaiveDate::from_ymd_opt(2026, 7, 17).unwrap();
        let running: std::collections::HashSet<String> = ["sess-A".to_string()].into_iter().collect();

        let snap = c.snapshot(Window::All, today, &running);

        // by_instance is scoped to the running set: only sess-A shows up.
        assert_eq!(snap.by_instance.len(), 1);
        assert_eq!(snap.by_instance[0].session_id, "sess-A");

        // by_model is unaffected by the running filter: both files' tokens
        // are aggregated together (110 + 44 = 154).
        assert_eq!(snap.by_model.len(), 1);
        assert_eq!(snap.by_model[0].toks.total(), 110 + 44);

        std::fs::remove_dir_all(&dir).ok();
    }
}
