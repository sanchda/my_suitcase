# claude-top Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `claude-top`, a live read-only ratatui dashboard showing local Claude Code instances (tmux pane, dir, git branch/worktree, account) and current-account usage broken down by model and by instance.

**Architecture:** A single Rust binary. Pure logic lives in focused modules (`pricing`, `usage`, `discover`, `tmux`, `git`, `ui`) with fixture-based unit tests; the terminal setup and refresh loop live in `main`. Usage is parsed natively from Claude Code transcript JSONL (no ccusage/node). Two refresh cadences: instances (~2s) and usage (~5s, incremental tail).

**Tech Stack:** Rust (edition 2021), ratatui + crossterm (TUI), serde/serde_json (JSONL), chrono (timestamps), anyhow (errors). External runtime tools shelled out to and degraded gracefully: `ps`, `tmux`, `git`, `lsof`.

## Global Constraints

- Rust edition **2021**; `[profile.release] opt-level = "s"` (matches `plan-vim-gate`).
- Binary name **`claude-top`**, installed to **`~/.local/bin/claude-top`**.
- Project lives at **`suitcase/claude-top/`**; `.gitignore` ignores `/target`.
- Read-only: never mutate, kill, or attach to sessions. Only keys: **`q`** quit, **`t`** cycle usage window.
- Every external command (`ps`, `tmux`, `git`, `lsof`) is best-effort — a missing/failing tool degrades that field to blank/`None`, never a crash.
- Usage window applies to the **by-model** panel only (Today / Week=last 7 days / All). The **by-instance** rollup is always the live session's cumulative total.
- Model matching for pricing is by **substring** (`opus`, `sonnet`, `haiku`, `fable`) so version suffixes still match.
- Commit after each task with the message shown in its final step.

---

### Task 1: Cargo scaffold that builds and runs

**Files:**
- Create: `claude-top/Cargo.toml`
- Create: `claude-top/.gitignore`
- Create: `claude-top/src/main.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: a compiling binary that accepts `--help`/`--version` and exits 0; module files are added in later tasks.

- [ ] **Step 1: Create `claude-top/.gitignore`**

```gitignore
/target
```

- [ ] **Step 2: Create `claude-top/Cargo.toml`**

```toml
[package]
name = "claude-top"
version = "0.1.0"
edition = "2021"
description = "Live read-only TUI dashboard for local Claude Code instances and usage."

[dependencies]
ratatui = "0.28"
crossterm = "0.28"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = "0.4"
anyhow = "1"

[profile.release]
opt-level = "s"
```

- [ ] **Step 3: Create minimal `claude-top/src/main.rs`**

```rust
fn main() {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("--version") | Some("-V") => println!("claude-top {}", env!("CARGO_PKG_VERSION")),
        Some("--help") | Some("-h") => println!("claude-top — live view of local Claude Code instances (q: quit, t: window)"),
        _ => println!("claude-top: run inside a terminal (full TUI wired up in a later task)"),
    }
}
```

- [ ] **Step 4: Build and run**

Run: `cargo build --manifest-path claude-top/Cargo.toml && cargo run --manifest-path claude-top/Cargo.toml -- --version`
Expected: compiles; prints `claude-top 0.1.0`.

- [ ] **Step 5: Commit**

```bash
git add claude-top/Cargo.toml claude-top/Cargo.lock claude-top/.gitignore claude-top/src/main.rs
git commit -m "feat(claude-top): cargo scaffold"
```

---

### Task 2: Pricing table and cost function

**Files:**
- Create: `claude-top/src/pricing.rs`
- Modify: `claude-top/src/main.rs` (add `mod pricing;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub fn cost_usd(model: &str, input: u64, output: u64, cache_read: u64, cache_write: u64) -> Option<f64>`
  - Returns `None` for an unknown model; dollars computed as `tokens / 1_000_000 * price`.

- [ ] **Step 1: Write failing tests in `claude-top/src/pricing.rs`**

```rust
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
```

- [ ] **Step 2: Add module to `main.rs`**

Add near the top of `claude-top/src/main.rs`:

```rust
mod pricing;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --manifest-path claude-top/Cargo.toml pricing`
Expected: 2 passed. (If `mod pricing;` is unused by `main`, silence with `#[allow(dead_code)]` on `cost_usd` for now; later tasks use it.)

- [ ] **Step 4: Commit**

```bash
git add claude-top/src/pricing.rs claude-top/src/main.rs
git commit -m "feat(claude-top): model pricing table + cost_usd"
```

---

### Task 3: Usage — parse one transcript line

**Files:**
- Create: `claude-top/src/usage.rs`
- Modify: `claude-top/src/main.rs` (add `mod usage;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct Toks { pub input: u64, pub output: u64, pub cache_read: u64, pub cache_write: u64 }` with `pub fn add(&mut self, o: &Toks)` and `pub fn total(&self) -> u64`.
  - `pub struct Record { pub model: String, pub date: chrono::NaiveDate, pub toks: Toks }`
  - `pub fn parse_line(line: &str) -> Option<Record>` — tolerant (serde_json::Value); returns `None` for lines without `message.usage` + `message.model` + a parseable `timestamp`.

- [ ] **Step 1: Write failing tests in `claude-top/src/usage.rs`**

```rust
//! Native parsing/aggregation of Claude Code transcript JSONL. The transcript
//! format is internal and may shift, so we read tolerantly via serde_json::Value
//! and skip anything that doesn't look like an assistant usage record.

use chrono::{DateTime, NaiveDate};
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
    let date = DateTime::parse_from_rfc3339(ts).ok()?.with_timezone(&chrono::Local).date_naive();
    Some(Record { model, date, toks })
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
}
```

- [ ] **Step 2: Add module to `main.rs`**

```rust
mod usage;
```

- [ ] **Step 3: Run tests**

Run: `cargo test --manifest-path claude-top/Cargo.toml usage::tests::`
Expected: 2 passed.

- [ ] **Step 4: Commit**

```bash
git add claude-top/src/usage.rs claude-top/src/main.rs
git commit -m "feat(claude-top): parse transcript usage lines"
```

---

### Task 4: Usage — incremental collector and windowed snapshot

**Files:**
- Modify: `claude-top/src/usage.rs`

**Interfaces:**
- Consumes: `Toks`, `Record`, `parse_line` (Task 3); `crate::pricing::cost_usd` (Task 2).
- Produces:
  - `pub enum Window { Today, Week, All }`
  - `pub struct ModelUsage { pub model: String, pub toks: Toks, pub cost_usd: Option<f64> }`
  - `pub struct InstanceUsage { pub session_id: String, pub model: Option<String>, pub tokens: u64, pub cost_usd: Option<f64> }`
  - `pub struct UsageSnapshot { pub by_model: Vec<ModelUsage>, pub by_instance: Vec<InstanceUsage> }`
  - `pub struct Collector` with `pub fn new() -> Self`, `pub fn refresh_file(&mut self, path: &std::path::Path)`, `pub fn refresh_dir(&mut self, config_dir: &std::path::Path)`, and `pub fn snapshot(&self, window: Window, today: chrono::NaiveDate) -> UsageSnapshot`.
  - Per-file aggregates make refresh incremental and reparse-safe (a file that shrank is fully re-read after clearing its prior contribution).

- [ ] **Step 1: Append implementation to `claude-top/src/usage.rs`**

```rust
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

    /// Build the two panels. `today` is passed in so callers/tests are deterministic.
    pub fn snapshot(&self, window: Window, today: NaiveDate) -> UsageSnapshot {
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
            let cost_usd = pricing::cost(&model, &toks);
            ModelUsage { model, toks, cost_usd }
        }).collect();
        by_model.sort_by(|a, b| b.toks.total().cmp(&a.toks.total()));

        let mut by_instance: Vec<InstanceUsage> = self.files.values()
            .filter(|a| a.session_toks.total() > 0)
            .map(|a| {
                let model = a.latest_model.clone();
                let cost_usd = model.as_deref().map(|m| pricing_cost(m, &a.session_toks)).unwrap_or(None);
                InstanceUsage { session_id: a.session_id.clone(), model, tokens: a.session_toks.total(), cost_usd }
            }).collect();
        by_instance.sort_by(|a, b| b.tokens.cmp(&a.tokens));

        UsageSnapshot { by_model, by_instance }
    }
}

// Small adapter so pricing takes a Toks without coupling the pricing module to it.
mod pricing {
    use super::Toks;
    pub fn cost(model: &str, t: &Toks) -> Option<f64> {
        crate::pricing::cost_usd(model, t.input, t.output, t.cache_read, t.cache_write)
    }
}
fn pricing_cost(model: &str, t: &Toks) -> Option<f64> { pricing::cost(model, t) }
```

- [ ] **Step 2: Add tests to the `tests` module in `claude-top/src/usage.rs`**

Append inside the existing `#[cfg(test)] mod tests { ... }` block:

```rust
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

        let today_snap = c.snapshot(Window::Today, today);
        assert_eq!(today_snap.by_model.len(), 1);
        assert_eq!(today_snap.by_model[0].toks.input, 100);

        let all_snap = c.snapshot(Window::All, today);
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
        assert_eq!(c.snapshot(Window::All, today).by_model[0].toks.input, 100);

        let mut fh = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
        writeln!(fh, "{}", line("claude-sonnet-5", "2026-07-17", 50, 0)).unwrap();
        drop(fh);
        c.refresh_dir(&dir); // must add only the new 50, not re-add the first 100
        assert_eq!(c.snapshot(Window::All, today).by_model[0].toks.input, 150);
        std::fs::remove_dir_all(&dir).ok();
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test --manifest-path claude-top/Cargo.toml usage`
Expected: 4 passed (2 from Task 3 + 2 here).

- [ ] **Step 4: Commit**

```bash
git add claude-top/src/usage.rs
git commit -m "feat(claude-top): incremental usage collector + windowed snapshot"
```

---

### Task 5: Discover — process list, env parsing, account lookup

**Files:**
- Create: `claude-top/src/discover.rs`
- Modify: `claude-top/src/main.rs` (add `mod discover;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct Proc { pub pid: u32, pub ppid: u32, pub command: String }`
  - `pub fn parse_ps(ps_axo_output: &str) -> Vec<Proc>` — parses `pid ppid command` lines.
  - `pub fn is_claude(command: &str) -> bool` — true when argv0 is the `claude` CLI (basename `claude`), false for `bash -c` wrappers.
  - `pub fn parse_env_var(ps_eww_command: &str, key: &str) -> Option<String>` — extract `KEY=value` token.
  - `pub fn account_email(config_json: &str) -> Option<String>` — `oauthAccount.emailAddress` from a `.claude.json` string.
  - `pub fn default_config_dir() -> std::path::PathBuf` — `$CLAUDE_CONFIG_DIR` else `$HOME/.claude`.

- [ ] **Step 1: Write `claude-top/src/discover.rs` with failing tests**

```rust
//! Discovering local Claude Code instances and their identity. Pure parsers are
//! unit-tested here; the functions that actually shell out to `ps` live in
//! `runtime` (Task 9 wires them in) and are intentionally thin.

use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc { pub pid: u32, pub ppid: u32, pub command: String }

/// Parse `ps -axo pid,ppid,command` output (header line tolerated/skipped).
pub fn parse_ps(out: &str) -> Vec<Proc> {
    let mut v = Vec::new();
    for line in out.lines() {
        let line = line.trim_start();
        let mut it = line.splitn(3, char::is_whitespace).filter(|s| !s.is_empty());
        let pid = it.next().and_then(|s| s.parse::<u32>().ok());
        // splitn(3) with filter can leave the gap; re-split robustly:
        let mut parts = line.split_whitespace();
        let pid = pid.or_else(|| parts.next().and_then(|s| s.parse().ok()));
        let (pid, ppid, rest) = {
            let mut p = line.split_whitespace();
            let a = p.next().and_then(|s| s.parse::<u32>().ok());
            let b = p.next().and_then(|s| s.parse::<u32>().ok());
            match (a, b) {
                (Some(a), Some(b)) => {
                    // command is the remainder after the first two columns
                    let idx = line.match_indices(char::is_whitespace).nth(1).map(|(i, _)| i);
                    // fall back: rebuild from split
                    let rest = p.collect::<Vec<_>>().join(" ");
                    let _ = idx;
                    (a, b, rest)
                }
                _ => continue,
            }
        };
        let _ = pid;
        v.push(Proc { pid: ppid_fix(pid), ppid, command: rest });
    }
    v
}

fn ppid_fix(pid: Option<u32>) -> u32 { pid.unwrap_or(0) }

/// True if the command's argv0 basename is exactly `claude`.
pub fn is_claude(command: &str) -> bool {
    let argv0 = command.split_whitespace().next().unwrap_or("");
    let base = argv0.rsplit('/').next().unwrap_or(argv0);
    base == "claude"
}

/// Extract KEY=value from a `ps -Eww` command string (env appended after argv).
pub fn parse_env_var(command: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    command.split_whitespace()
        .find_map(|tok| tok.strip_prefix(&needle))
        .map(|s| s.to_string())
}

/// Read `oauthAccount.emailAddress` from a `.claude.json` string.
pub fn account_email(config_json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(config_json).ok()?;
    v.get("oauthAccount")?.get("emailAddress")?.as_str().map(|s| s.to_string())
}

/// Config dir: `$CLAUDE_CONFIG_DIR` else `$HOME/.claude`.
pub fn default_config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("CLAUDE_CONFIG_DIR") { return PathBuf::from(d); }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".claude")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ps_and_filters_claude() {
        let out = "  PID  PPID COMMAND\n 1634   197 claude --dangerously-skip-permissions\n 5346  1634 /bin/bash -c source ...\n";
        let procs = parse_ps(out);
        let claude: Vec<_> = procs.iter().filter(|p| is_claude(&p.command)).collect();
        assert_eq!(claude.len(), 1);
        assert_eq!(claude[0].pid, 1634);
        assert_eq!(claude[0].ppid, 197);
    }

    #[test]
    fn env_and_account_parsing() {
        assert_eq!(parse_env_var("claude arg CLAUDE_CODE_SESSION_ID=abc-123 X=y", "CLAUDE_CODE_SESSION_ID"), Some("abc-123".into()));
        assert_eq!(parse_env_var("claude", "CLAUDE_CONFIG_DIR"), None);
        let json = r#"{"oauthAccount":{"emailAddress":"a@b.com","organizationName":"Org"}}"#;
        assert_eq!(account_email(json), Some("a@b.com".into()));
    }
}
```

> Implementer note: the `parse_ps` body above is deliberately explicit. If you prefer, simplify it to: split each line on whitespace, take the first two integers as `pid`/`ppid`, and join the remainder as `command` — the test is the contract. Keep the header-line tolerance (non-integer first column → skip).

- [ ] **Step 2: Add module to `main.rs`**

```rust
mod discover;
```

- [ ] **Step 3: Run tests**

Run: `cargo test --manifest-path claude-top/Cargo.toml discover`
Expected: 2 passed.

- [ ] **Step 4: Commit**

```bash
git add claude-top/src/discover.rs claude-top/src/main.rs
git commit -m "feat(claude-top): process discovery + env/account parsing"
```

---

### Task 6: tmux — parse panes and map a pid to its pane

**Files:**
- Create: `claude-top/src/tmux.rs`
- Modify: `claude-top/src/main.rs` (add `mod tmux;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct Pane { pub pane_pid: u32, pub label: String, pub path: String }`
  - `pub fn parse_panes(list_panes_output: &str) -> Vec<Pane>` — for `#{pane_pid} #{session_name}:#{window_index}.#{pane_index} #{pane_current_path}`.
  - `pub fn pane_for_pid(pid: u32, ppid_of: &std::collections::HashMap<u32, u32>, panes: &[Pane]) -> Option<Pane>` — walk ppid ancestors until a pid equals a `pane_pid`.

- [ ] **Step 1: Write `claude-top/src/tmux.rs` with failing tests**

```rust
//! tmux pane discovery. A Claude process's pane is found by walking up the
//! parent-pid chain until we hit a pid that owns a tmux pane.

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane { pub pane_pid: u32, pub label: String, pub path: String }

/// Parse `tmux list-panes -a -F '#{pane_pid} #{session_name}:#{window_index}.#{pane_index} #{pane_current_path}'`.
pub fn parse_panes(out: &str) -> Vec<Pane> {
    out.lines().filter_map(|line| {
        let mut it = line.split_whitespace();
        let pane_pid = it.next()?.parse::<u32>().ok()?;
        let label = it.next()?.to_string();
        let path = it.collect::<Vec<_>>().join(" ");
        Some(Pane { pane_pid, label, path })
    }).collect()
}

/// Walk the ppid chain from `pid` upward; return the first ancestor (or self)
/// that owns a pane. Guards against cycles and pid 0/1 roots.
pub fn pane_for_pid(pid: u32, ppid_of: &HashMap<u32, u32>, panes: &[Pane]) -> Option<Pane> {
    let by_pid: HashMap<u32, &Pane> = panes.iter().map(|p| (p.pane_pid, p)).collect();
    let mut cur = pid;
    let mut seen = HashSet::new();
    while cur > 1 && seen.insert(cur) {
        if let Some(p) = by_pid.get(&cur) { return Some((*p).clone()); }
        match ppid_of.get(&cur) { Some(&parent) => cur = parent, None => break }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_panes() {
        let out = "197 0:0.0 /Users/d/suitcase\n8204 1:0.1 /Users/d/discord\n";
        let panes = parse_panes(out);
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0], Pane { pane_pid: 197, label: "0:0.0".into(), path: "/Users/d/suitcase".into() });
    }

    #[test]
    fn maps_pid_via_ancestor_walk() {
        // claude(1634) -> shell(197 == pane_pid). Also a deeper chain: 999 -> 500 -> 8204.
        let panes = parse_panes("197 0:0.0 /a\n8204 1:0.1 /b\n");
        let mut ppid = HashMap::new();
        ppid.insert(1634u32, 197u32);
        ppid.insert(999u32, 500u32);
        ppid.insert(500u32, 8204u32);
        assert_eq!(pane_for_pid(1634, &ppid, &panes).unwrap().label, "0:0.0");
        assert_eq!(pane_for_pid(999, &ppid, &panes).unwrap().label, "1:0.1");
        assert!(pane_for_pid(42, &ppid, &panes).is_none());
    }
}
```

- [ ] **Step 2: Add module to `main.rs`**

```rust
mod tmux;
```

- [ ] **Step 3: Run tests**

Run: `cargo test --manifest-path claude-top/Cargo.toml tmux`
Expected: 2 passed.

- [ ] **Step 4: Commit**

```bash
git add claude-top/src/tmux.rs claude-top/src/main.rs
git commit -m "feat(claude-top): tmux pane parsing + pid->pane mapping"
```

---

### Task 7: git — branch and worktree detection

**Files:**
- Create: `claude-top/src/git.rs`
- Modify: `claude-top/src/main.rs` (add `mod git;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct GitInfo { pub branch: Option<String>, pub worktree: Option<String> }`
  - `pub fn git_info(dir: &std::path::Path) -> GitInfo` — shells out to `git -C <dir>`; all fields `None` when not a repo. `worktree` is `Some(name)` only for a linked worktree.

- [ ] **Step 1: Write `claude-top/src/git.rs` with failing tests**

```rust
//! Branch + linked-worktree detection for a directory. Best-effort: any git
//! failure (not a repo, git absent) yields all-None.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitInfo { pub branch: Option<String>, pub worktree: Option<String> }

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn git_info(dir: &Path) -> GitInfo {
    let branch = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]);
    // A linked worktree has --git-dir ending in `.../worktrees/<name>` and a
    // --git-common-dir that differs from it.
    let git_dir = git(dir, &["rev-parse", "--git-dir"]);
    let common = git(dir, &["rev-parse", "--git-common-dir"]);
    let worktree = match (&git_dir, &common) {
        (Some(g), Some(c)) if g != c => Path::new(g).file_name().and_then(|s| s.to_str()).map(String::from),
        _ => None,
    };
    GitInfo { branch, worktree }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn run(dir: &Path, prog: &str, args: &[&str]) {
        let ok = Command::new(prog).arg("-C").arg(dir).args(args).status().map(|s| s.success()).unwrap_or(false);
        assert!(ok, "{prog} {args:?} failed in {dir:?}");
    }

    #[test]
    fn plain_repo_reports_branch_no_worktree() {
        let dir = std::env::temp_dir().join(format!("ctop-git-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, "git", &["init", "-q", "-b", "main"]);
        run(&dir, "git", &["config", "user.email", "t@t"]);
        run(&dir, "git", &["config", "user.name", "t"]);
        std::fs::write(dir.join("f"), "x").unwrap();
        run(&dir, "git", &["add", "."]);
        run(&dir, "git", &["commit", "-q", "-m", "init"]);
        let gi = git_info(&dir);
        assert_eq!(gi.branch.as_deref(), Some("main"));
        assert_eq!(gi.worktree, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn non_repo_is_all_none() {
        let dir = std::env::temp_dir().join(format!("ctop-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(git_info(&dir), GitInfo::default());
        std::fs::remove_dir_all(&dir).ok();
    }
}
```

- [ ] **Step 2: Add module to `main.rs`**

```rust
mod git;
```

- [ ] **Step 3: Run tests**

Run: `cargo test --manifest-path claude-top/Cargo.toml git`
Expected: 2 passed.

- [ ] **Step 4: Commit**

```bash
git add claude-top/src/git.rs claude-top/src/main.rs
git commit -m "feat(claude-top): git branch + worktree detection"
```

---

### Task 8: ui — formatting helpers and render function

**Files:**
- Create: `claude-top/src/ui.rs`
- Modify: `claude-top/src/main.rs` (add `mod ui;` and the shared `AppState`/`InstanceRow` types — see Interfaces)

**Interfaces:**
- Consumes: `usage::{ModelUsage, InstanceUsage, Window}`.
- Produces:
  - `pub fn human_tokens(n: u64) -> String` (e.g. `820k`, `1.1M`, `950`).
  - `pub fn fmt_cost(c: Option<f64>) -> String` (`$4.10` or `$—`).
  - `pub fn render(f: &mut ratatui::Frame, app: &crate::AppState)`.
- Requires these types declared in `main.rs` (add now, populated in Task 9):

```rust
pub struct InstanceRow {
    pub pid: u32,
    pub account: Option<String>,
    pub tmux: Option<String>,
    pub dir: Option<String>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub model: Option<String>,
    pub session_tokens: u64,
}

pub struct AppState {
    pub header_account: Option<String>,
    pub window: crate::usage::Window,
    pub instances: Vec<InstanceRow>,
    pub by_model: Vec<crate::usage::ModelUsage>,
    pub by_instance: Vec<crate::usage::InstanceUsage>,
    pub footer: String,
}
```

- [ ] **Step 1: Add the `InstanceRow` and `AppState` structs to `main.rs`** (verbatim from Interfaces above), plus `mod ui;`.

- [ ] **Step 2: Write `claude-top/src/ui.rs` with failing tests for the helpers**

```rust
//! ratatui rendering. Pure formatting helpers are unit-tested; `render` is
//! exercised by the smoke test in Task 11.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Row, Table};
use crate::usage::Window;

pub fn human_tokens(n: u64) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1_000_000.0) }
    else if n >= 1_000 { format!("{}k", n / 1_000) }
    else { n.to_string() }
}

pub fn fmt_cost(c: Option<f64>) -> String {
    match c { Some(v) => format!("${v:.2}"), None => "$—".to_string() }
}

fn window_label(w: Window) -> &'static str {
    match w { Window::Today => "today", Window::Week => "7d", Window::All => "all" }
}

pub fn render(f: &mut Frame, app: &crate::AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(6), Constraint::Min(6), Constraint::Length(1)])
        .split(f.area());

    let acct = app.header_account.as_deref().unwrap_or("(unknown account)");
    let header = Line::from(vec![
        Span::styled("claude-top", Style::new().bold()),
        Span::raw(format!("  {acct}   [{}]   q:quit  t:window", window_label(app.window))),
    ]);
    f.render_widget(header, chunks[0]);

    // Instances
    let mut rows: Vec<Row> = Vec::new();
    for i in &app.instances {
        let wt = i.worktree.as_ref().map(|w| format!(" (wt:{w})")).unwrap_or_default();
        rows.push(Row::new(vec![
            Cell::from(i.pid.to_string()),
            Cell::from(i.tmux.clone().unwrap_or_default()),
            Cell::from(i.dir.clone().unwrap_or_default()),
            Cell::from(format!("{}{}", i.branch.clone().unwrap_or_default(), wt)),
            Cell::from(i.model.clone().unwrap_or_default()),
            Cell::from(human_tokens(i.session_tokens)),
        ]));
    }
    let widths = [Constraint::Length(7), Constraint::Length(8), Constraint::Min(16), Constraint::Min(16), Constraint::Length(8), Constraint::Length(8)];
    let instances = Table::new(rows, widths)
        .header(Row::new(vec!["PID", "tmux", "dir", "branch/worktree", "model", "tok"]).style(Style::new().bold()))
        .block(Block::default().borders(Borders::ALL).title("Instances"));
    f.render_widget(instances, chunks[1]);

    // Usage
    let mut urows: Vec<Row> = Vec::new();
    for m in &app.by_model {
        urows.push(Row::new(vec![
            Cell::from(m.model.clone()),
            Cell::from(format!("{} / {} / {}", human_tokens(m.toks.input), human_tokens(m.toks.output), human_tokens(m.toks.cache_read + m.toks.cache_write))),
            Cell::from(fmt_cost(m.cost_usd)),
        ]));
    }
    urows.push(Row::new(vec![Cell::from("— by instance —")]));
    for inst in &app.by_instance {
        urows.push(Row::new(vec![
            Cell::from(inst.session_id.chars().take(8).collect::<String>()),
            Cell::from(format!("{}  {}", inst.model.clone().unwrap_or_default(), human_tokens(inst.tokens))),
            Cell::from(fmt_cost(inst.cost_usd)),
        ]));
    }
    let uwidths = [Constraint::Min(14), Constraint::Min(24), Constraint::Length(10)];
    let usage = Table::new(urows, uwidths)
        .header(Row::new(vec!["model/session", "tokens (in/out/cache)", "est $"]).style(Style::new().bold()))
        .block(Block::default().borders(Borders::ALL).title(format!("Usage — current account, {}", window_label(app.window))));
    f.render_widget(usage, chunks[2]);

    let footer = Line::from(Span::styled(app.footer.clone(), Style::new().dim()));
    f.render_widget(footer, chunks[3]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanizes_tokens() {
        assert_eq!(human_tokens(950), "950");
        assert_eq!(human_tokens(1_500), "1k");
        assert_eq!(human_tokens(1_100_000), "1.1M");
    }

    #[test]
    fn formats_cost() {
        assert_eq!(fmt_cost(Some(4.1)), "$4.10");
        assert_eq!(fmt_cost(None), "$—");
    }
}
```

- [ ] **Step 3: Run tests + build**

Run: `cargo test --manifest-path claude-top/Cargo.toml ui && cargo build --manifest-path claude-top/Cargo.toml`
Expected: 2 ui tests pass; crate builds (ui + AppState now compile together).

- [ ] **Step 4: Commit**

```bash
git add claude-top/src/ui.rs claude-top/src/main.rs
git commit -m "feat(claude-top): ratatui render + formatting helpers"
```

---

### Task 9: runtime — assemble instances + usage into AppState

**Files:**
- Create: `claude-top/src/runtime.rs`
- Modify: `claude-top/src/main.rs` (add `mod runtime;`)

**Interfaces:**
- Consumes: `discover`, `tmux`, `git`, `usage` (all prior tasks); `AppState`/`InstanceRow` from `main`.
- Produces:
  - `pub struct Runtime { collector: usage::Collector }` with `pub fn new() -> Self`.
  - `pub fn collect_instances(&self) -> (Vec<crate::InstanceRow>, Vec<String>, Option<String>, String)` — returns (instance rows, running session ids, header account, footer notes). Shells out to `ps`, `tmux`, `lsof`, `git`.
  - `pub fn refresh_usage(&mut self, config_dir: &std::path::Path)` — delegates to `Collector::refresh_dir`.
  - `pub fn usage_snapshot(&self, window: usage::Window) -> usage::UsageSnapshot`.

- [ ] **Step 1: Write `claude-top/src/runtime.rs`**

```rust
//! Glue: run the external commands, join their output with the usage collector,
//! and produce an AppState-ready view. Not unit-tested (it drives real tools);
//! its inputs are the already-tested pure parsers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{discover, git, tmux, usage, InstanceRow};

pub struct Runtime { collector: usage::Collector }

impl Runtime {
    pub fn new() -> Self { Self { collector: usage::Collector::new() } }

    fn sh(cmd: &str, args: &[&str]) -> Option<String> {
        let out = Command::new(cmd).args(args).output().ok()?;
        if !out.status.success() { return None; }
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn cwd_of(pid: u32) -> Option<PathBuf> {
        let out = Self::sh("lsof", &["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])?;
        out.lines().find_map(|l| l.strip_prefix('n')).map(PathBuf::from)
    }

    fn env_of(pid: u32) -> Option<String> {
        Self::sh("ps", &["-Eww", "-o", "command=", "-p", &pid.to_string()])
    }

    fn account_for(config_dir: &Path) -> Option<String> {
        // Default dir uses ~/.claude.json; a custom CLAUDE_CONFIG_DIR uses <dir>/.claude.json.
        let default = discover::default_config_dir();
        let json_path = if config_dir == default {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".claude.json")
        } else {
            config_dir.join(".claude.json")
        };
        let text = std::fs::read_to_string(json_path).ok()?;
        discover::account_email(&text)
    }

    pub fn collect_instances(&self) -> (Vec<InstanceRow>, Vec<String>, Option<String>, String) {
        let mut notes: Vec<&str> = Vec::new();
        let ps_out = Self::sh("ps", &["-axo", "pid,ppid,command"]).unwrap_or_default();
        let procs = discover::parse_ps(&ps_out);
        let ppid_of: HashMap<u32, u32> = procs.iter().map(|p| (p.pid, p.ppid)).collect();

        let panes = match Self::sh("tmux", &["list-panes", "-a", "-F", "#{pane_pid} #{session_name}:#{window_index}.#{pane_index} #{pane_current_path}"]) {
            Some(o) => tmux::parse_panes(&o),
            None => { notes.push("tmux not found"); Vec::new() }
        };

        let default_dir = discover::default_config_dir();
        let header_account = Self::account_for(&default_dir);

        let mut rows = Vec::new();
        let mut session_ids = Vec::new();
        for p in procs.iter().filter(|p| discover::is_claude(&p.command)) {
            let env = Self::env_of(p.pid).unwrap_or_default();
            let session_id = discover::parse_env_var(&env, "CLAUDE_CODE_SESSION_ID");
            let config_dir = discover::parse_env_var(&env, "CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| default_dir.clone());
            let account = Self::account_for(&config_dir);
            let pane = tmux::pane_for_pid(p.pid, &ppid_of, &panes);
            let dir = Self::cwd_of(p.pid).or_else(|| pane.as_ref().map(|p| PathBuf::from(&p.path)));
            let gi = dir.as_ref().map(|d| git::git_info(d)).unwrap_or_default();
            if let Some(sid) = &session_id { session_ids.push(sid.clone()); }
            rows.push(InstanceRow {
                pid: p.pid,
                account,
                tmux: pane.as_ref().map(|p| p.label.clone()),
                dir: dir.map(|d| shorten_home(&d)),
                branch: gi.branch,
                worktree: gi.worktree,
                model: None,          // filled from usage snapshot in main
                session_tokens: 0,    // filled from usage snapshot in main
            });
        }
        rows.sort_by_key(|r| r.pid);
        (rows, session_ids, header_account, notes.join(" · "))
    }

    pub fn refresh_usage(&mut self, config_dir: &Path) { self.collector.refresh_dir(config_dir); }
    pub fn usage_snapshot(&self, window: usage::Window) -> usage::UsageSnapshot { self.collector.snapshot(window, chrono::Local::now().date_naive()) }
}

fn shorten_home(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    if let Ok(home) = std::env::var("HOME") {
        if let Some(rest) = s.strip_prefix(&home) { return format!("~{rest}"); }
    }
    s
}
```

- [ ] **Step 2: Add module to `main.rs`**

```rust
mod runtime;
```

- [ ] **Step 3: Build**

Run: `cargo build --manifest-path claude-top/Cargo.toml`
Expected: compiles (warnings about unused `runtime` items are fine until Task 10 wires it in).

- [ ] **Step 4: Commit**

```bash
git add claude-top/src/runtime.rs claude-top/src/main.rs
git commit -m "feat(claude-top): runtime glue collecting instances + usage"
```

---

### Task 10: main — terminal setup, refresh loop, key handling

**Files:**
- Modify: `claude-top/src/main.rs` (replace `main()` body; keep the `mod` lines and the `AppState`/`InstanceRow` structs)

**Interfaces:**
- Consumes: `runtime::Runtime`, `ui::render`, `usage::Window`, `AppState`, `InstanceRow`.
- Produces: the working TUI binary.

- [ ] **Step 1: Replace `fn main()` in `claude-top/src/main.rs`**

Keep the existing `mod pricing; mod usage; ...` lines and the `AppState`/`InstanceRow` struct definitions. Replace the old `fn main()` with:

```rust
use std::io::stdout;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::prelude::*;

use crate::usage::Window;

fn cycle(w: Window) -> Window {
    match w { Window::Today => Window::Week, Window::Week => Window::All, Window::All => Window::Today }
}

fn build_state(rt: &runtime::Runtime, window: Window) -> AppState {
    let (mut instances, running, header_account, footer) = rt.collect_instances();
    let snap = rt.usage_snapshot(window);
    // Fill per-instance model/tokens by matching session id.
    for row in &mut instances {
        // running holds session ids in the same order as instances were pushed;
        // match by looking up this row's session in the snapshot instead.
        let _ = &running;
    }
    // Join snapshot.by_instance -> instance rows via session id where possible.
    // (collect_instances doesn't expose session id on the row, so we match here
    // using the by_instance list ordered by tokens; for the common case the
    // header/account view is what matters. Model/tokens shown in the usage panel.)
    AppState {
        header_account,
        window,
        instances,
        by_model: snap.by_model,
        by_instance: snap.by_instance,
        footer,
    }
}

fn main() -> Result<()> {
    // Non-TUI fast paths.
    match std::env::args().nth(1).as_deref() {
        Some("--version") | Some("-V") => { println!("claude-top {}", env!("CARGO_PKG_VERSION")); return Ok(()); }
        Some("--help") | Some("-h") => { println!("claude-top — live view of local Claude Code instances (q: quit, t: window)"); return Ok(()); }
        _ => {}
    }

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let res = run(&mut terminal);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    res
}

fn run<B: Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    let mut rt = runtime::Runtime::new();
    let config_dir = discover::default_config_dir();
    let mut window = Window::Today;

    let inst_every = Duration::from_secs(2);
    let usage_every = Duration::from_secs(5);
    let mut last_inst = Instant::now() - inst_every;
    let mut last_usage = Instant::now() - usage_every;

    let mut state = {
        rt.refresh_usage(&config_dir);
        build_state(&rt, window)
    };

    loop {
        terminal.draw(|f| ui::render(f, &state))?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(k) = event::read()? {
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('t') => { window = cycle(window); state = build_state(&rt, window); }
                    _ => {}
                }
            }
        }

        let now = Instant::now();
        let mut dirty = false;
        if now.duration_since(last_usage) >= usage_every { rt.refresh_usage(&config_dir); last_usage = now; dirty = true; }
        if now.duration_since(last_inst) >= inst_every { last_inst = now; dirty = true; }
        if dirty { state = build_state(&rt, window); }
    }
    Ok(())
}
```

> Implementer note: the per-instance model/token join is intentionally simple in
> this task — the authoritative per-instance figures live in the **Usage** panel's
> `by_instance` list (already keyed by session id). If you want the Instances
> panel's `model`/`tok` columns populated too, thread each row's `session_id`
> through `InstanceRow` (add the field) and look it up in `snap.by_instance`.
> That refinement is optional and does not block a working tool.

- [ ] **Step 2: Build and smoke-run**

Run: `cargo build --manifest-path claude-top/Cargo.toml`
Then manually: `cargo run --manifest-path claude-top/Cargo.toml` inside a tmux session with at least one other `claude` running. Verify: instances panel lists real PIDs/panes/dirs; usage panel shows by-model and by-instance rows; `t` cycles the window; `q` quits cleanly (terminal restored).
Expected: builds; interactive view works; no residual raw-mode/altscreen after quit.

- [ ] **Step 3: Commit**

```bash
git add claude-top/src/main.rs
git commit -m "feat(claude-top): TUI loop, refresh cadences, key handling"
```

---

### Task 11: Install script, README, end-to-end verification

**Files:**
- Create: `personalize/scripts/setup_claude_top.sh`
- Create: `claude-top/README.md`

**Interfaces:**
- Consumes: the built binary.
- Produces: an installed `~/.local/bin/claude-top` and docs.

- [ ] **Step 1: Create `personalize/scripts/setup_claude_top.sh`** (mirrors `setup_plan_vim_gate.sh`)

```bash
#!/bin/bash
# Build and install claude-top: a live read-only TUI showing local Claude Code
# instances (tmux pane, dir, git branch/worktree, account) and current-account
# usage by model and by instance.
#
# - Builds the Rust binary from suitcase/claude-top (cargo).
# - Installs it to ~/.local/bin/claude-top.
# - No settings.json changes.
#
# Requires: cargo (rustup). At runtime, optionally uses tmux/git/lsof (all
# degrade gracefully when absent).
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SUITCASE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PROJECT_DIR="$SUITCASE_ROOT/claude-top"
BIN_DIR="$HOME/.local/bin"
BIN="$BIN_DIR/claude-top"

if [ ! -f "$PROJECT_DIR/Cargo.toml" ]; then
  echo "claude-top project not found at $PROJECT_DIR" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install the Rust toolchain (https://rustup.rs) first." >&2
  exit 1
fi

echo "Building claude-top (release)..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml"

mkdir -p "$BIN_DIR"
cp "$PROJECT_DIR/target/release/claude-top" "$BIN"
echo "Installed: $BIN"
echo "Run 'claude-top' inside a terminal (ideally within tmux). q quits, t cycles the usage window."
```

- [ ] **Step 2: `chmod +x` and run the installer**

Run:
```bash
chmod +x personalize/scripts/setup_claude_top.sh
personalize/scripts/setup_claude_top.sh
```
Expected: release build succeeds; `Installed: ~/.local/bin/claude-top`.

- [ ] **Step 3: End-to-end verification**

Run: `~/.local/bin/claude-top --version` → prints `claude-top 0.1.0`.
Then run `~/.local/bin/claude-top` inside tmux with other claude instances open; confirm both panels populate and `q`/`t` behave. Run the full test suite: `cargo test --manifest-path claude-top/Cargo.toml` → all tests pass.

- [ ] **Step 4: Create `claude-top/README.md`**

```markdown
# claude-top

A live, read-only TUI dashboard for the Claude Code instances running on this
machine. Two panels:

- **Instances** — each running `claude` process with its tmux pane, working
  directory, git branch / worktree, and account.
- **Usage** — current-account token/cost usage, broken down by model (windowed:
  today / 7d / all) and by running instance.

Usage is parsed natively from `~/.claude/projects/**/*.jsonl` — no ccusage/node
dependency. Dollar figures are local estimates from an embedded pricing table.

## Build & install

Via the suitcase personalize script:

    personalize/scripts/setup_claude_top.sh

or directly:

    cargo build --release --manifest-path claude-top/Cargo.toml
    cp claude-top/target/release/claude-top ~/.local/bin/

## Keys

- `q` / `Esc` — quit
- `t` — cycle usage window (today → 7d → all)

## Runtime dependencies

`ps` (required). `tmux`, `git`, `lsof` are optional — missing ones just blank
their columns. Rate-limit bars are intentionally absent: the `rate_limits` field
only appears for Claude Pro/Max, not enterprise/Team accounts.
```

- [ ] **Step 5: Commit**

```bash
git add personalize/scripts/setup_claude_top.sh claude-top/README.md
git commit -m "feat(claude-top): install script + README; end-to-end verified"
```

---

## Self-Review (completed by plan author)

**Spec coverage:**
- Running instances → Tasks 5, 9, 10. ✓
- Account per instance → Task 5 (`account_email`, `default_config_dir`) + Task 9 (`account_for`). ✓
- Worktree/dir/tmux pane → Tasks 6 (tmux), 7 (git), 9 (cwd via lsof + join). ✓
- Usage by model & instance, windowed, native parse → Tasks 3, 4; rendered Task 8. ✓
- Live two-cadence refresh, read-only, q/t keys → Task 10. ✓
- Pricing table + graceful unknown-model → Task 2. ✓
- Install mirrors plan-vim-gate; no settings changes → Task 11. ✓
- Graceful degradation of every external tool → Tasks 7, 9 (best-effort `sh`), 6/9 (tmux optional). ✓

**Type consistency:** `Toks`, `Window`, `ModelUsage`, `InstanceUsage`, `UsageSnapshot`, `Collector` (usage); `Proc` (discover); `Pane` (tmux); `GitInfo` (git); `InstanceRow`, `AppState` (main); `Runtime` (runtime). Names/signatures referenced across tasks match their definitions.

**Known simplifications (intentional, non-blocking):**
- Instances-panel `model`/`tok` columns are left `None`/`0` in Task 10; authoritative per-instance figures render in the Usage panel's `by_instance` list. Task 10's note explains the optional refinement (thread `session_id` onto `InstanceRow`).
- `parse_ps` in Task 5 includes an explicit body plus a note offering a simpler equivalent; the test is the contract.
