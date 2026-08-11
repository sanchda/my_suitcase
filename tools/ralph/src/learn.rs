//! `ralph learn` — mine the run log for durable, non-obvious lessons and keep
//! them as one file per learning under `.ralph/learnings/`.
//!
//! Two-phase, propose-then-approve (the miner never writes learnings itself):
//!   ralph learn                 mine → print numbered proposals, save them
//!   ralph learn --apply         write every saved proposal to learnings/
//!   ralph learn --apply 1,3     write a subset
//!   ralph learn --discard       drop saved proposals
//!
//! Discipline: if nothing non-obvious was learned, propose nothing; existing
//! learnings are shown to the miner so it doesn't re-propose them; one
//! learning per file so cleanup is `rm`. Learnings are injected (byte-capped)
//! into every iteration prompt.

use crate::config::Config;
use crate::{synth, R};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Wall-clock cap for the one-shot mining call.
const LEARN_TIMEOUT_SECS: u64 = 180;
/// How much run.log tail the miner sees.
const RUN_LOG_TAIL_BYTES: usize = 24 * 1024;
/// Byte cap on the learnings block injected into each iteration prompt.
const INJECT_TOTAL_BYTES: usize = 4 * 1024;
/// Byte cap per learning file when injecting.
const INJECT_FILE_BYTES: usize = 1024;

/// One proposed (or stored) learning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Learning {
    pub slug: String,
    pub title: String,
    pub body: String,
}

pub fn learnings_dir(cfg: &Config) -> PathBuf {
    cfg.dir.join("learnings")
}

fn proposals_path(cfg: &Config) -> PathBuf {
    cfg.dir.join("learn-proposals.json")
}

/// Existing learning titles (first `# ` heading per file), for dedup context.
pub fn existing_titles(dir: &Path) -> Vec<String> {
    let mut titles = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return titles;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    paths.sort();
    for path in paths {
        if let Ok(text) = fs::read_to_string(&path) {
            let title = text
                .lines()
                .find_map(|l| l.strip_prefix("# "))
                .unwrap_or_else(|| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("untitled")
                })
                .to_string();
            titles.push(title);
        }
    }
    titles
}

/// Assemble the mining prompt. Pure for testing.
pub fn build_mine_prompt(run_log_tail: &str, carry_forward: &str, existing: &[String]) -> String {
    let mut p = String::from(
        "You mine an autonomous coding loop's run log for durable, non-obvious \
         lessons worth carrying into FUTURE runs and prompts: recurring failure \
         patterns, environment/toolchain gotchas, verification traps, model-tier \
         lessons. Skip anything obvious, one-off, task-specific, or already \
         covered by the existing learnings listed below. High bar: a learning \
         must plausibly change how a future iteration or PROMPT.md is written.\n\n\
         Output STRICT JSON only — an array of objects with exactly these keys:\n\
         [{\"slug\": \"kebab-case-short-name\", \"title\": \"one line\", \
         \"body\": \"2-5 sentences: the lesson, why, and how to apply it\"}]\n\
         If nothing qualifies, output []. No prose, no code fences.\n\n",
    );
    p.push_str("## Existing learnings (do not re-propose)\n");
    if existing.is_empty() {
        p.push_str("(none)\n");
    } else {
        for t in existing {
            p.push_str("- ");
            p.push_str(t);
            p.push('\n');
        }
    }
    p.push_str("\n## Current carry-forward\n");
    p.push_str(if carry_forward.trim().is_empty() {
        "(none)"
    } else {
        carry_forward.trim()
    });
    p.push_str("\n\n## Run log (tail)\n");
    p.push_str(run_log_tail.trim());
    p.push('\n');
    p
}

/// Parse the miner's output: strict JSON array, tolerating a fenced wrapper.
/// Invalid entries are dropped; a non-array is an error (shown raw to the user).
pub fn parse_proposals(raw: &str) -> Result<Vec<Learning>, String> {
    let mut text = raw.trim();
    if let Some(rest) = text.strip_prefix("```") {
        // ```json\n...\n``` — cut the fence lines.
        let rest = rest.trim_start_matches("json").trim_start();
        text = rest.strip_suffix("```").map(str::trim).unwrap_or(rest);
    }
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("miner output is not JSON: {e}"))?;
    let items = value
        .as_array()
        .ok_or_else(|| "miner output is not a JSON array".to_string())?;
    let mut out = Vec::new();
    for item in items {
        let field = |k: &str| {
            item.get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        if let (Some(slug), Some(title), Some(body)) = (field("slug"), field("title"), field("body"))
        {
            out.push(Learning {
                slug: sanitize_slug(slug),
                title: title.to_string(),
                body: body.to_string(),
            });
        }
    }
    Ok(out)
}

/// Lowercase kebab: keep [a-z0-9], collapse everything else to single hyphens.
pub fn sanitize_slug(raw: &str) -> String {
    let mut out = String::new();
    let mut last_hyphen = true; // suppress a leading hyphen
    for ch in raw.to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_hyphen = false;
        } else if !last_hyphen {
            out.push('-');
            last_hyphen = true;
        }
    }
    let trimmed = out.trim_end_matches('-').to_string();
    if trimmed.is_empty() {
        "learning".to_string()
    } else {
        trimmed
    }
}

/// Write one learning as `<dir>/<slug>.md`, suffixing `-2`, `-3`, … when the
/// slug is taken. Returns the path written.
pub fn write_learning(dir: &Path, learning: &Learning) -> R<PathBuf> {
    fs::create_dir_all(dir)?;
    let mut path = dir.join(format!("{}.md", learning.slug));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("{}-{n}.md", learning.slug));
    }
    fs::write(&path, format!("# {}\n\n{}\n", learning.title, learning.body))?;
    Ok(path)
}

/// The `## Learnings` block injected into each iteration prompt: every file in
/// `<dir>` (sorted by name), each byte-capped, until the total budget runs out.
/// Empty string when there are no learnings — the prompt gains nothing.
pub fn injection_block(dir: &Path) -> String {
    let Ok(entries) = fs::read_dir(dir) else {
        return String::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    if paths.is_empty() {
        return String::new();
    }
    paths.sort();
    let mut out = String::from("\n\n## Learnings (durable lessons from earlier runs)\n\n");
    let mut used = 0usize;
    let mut skipped = 0usize;
    for path in &paths {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let mut entry = text.trim().to_string();
        if entry.len() > INJECT_FILE_BYTES {
            let mut end = INJECT_FILE_BYTES;
            while !entry.is_char_boundary(end) {
                end -= 1;
            }
            entry.truncate(end);
            entry.push('…');
        }
        if used + entry.len() > INJECT_TOTAL_BYTES {
            skipped += 1;
            continue;
        }
        used += entry.len();
        out.push_str(&entry);
        out.push_str("\n\n");
    }
    if skipped > 0 {
        // Name what was dropped rather than silently shrinking the prompt.
        out.push_str(&format!(
            "({skipped} more learning file(s) omitted — over the {INJECT_TOTAL_BYTES}-byte budget; prune {})\n",
            dir.display()
        ));
    }
    out
}

fn read_tail(path: &Path, max_bytes: usize) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    if text.len() <= max_bytes {
        return text;
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

fn save_proposals(cfg: &Config, proposals: &[Learning]) -> R<()> {
    let value: Vec<Value> = proposals
        .iter()
        .map(|l| {
            serde_json::json!({"slug": l.slug, "title": l.title, "body": l.body})
        })
        .collect();
    fs::write(
        proposals_path(cfg),
        serde_json::to_string_pretty(&Value::Array(value))?,
    )?;
    Ok(())
}

fn load_proposals(cfg: &Config) -> R<Vec<Learning>> {
    let path = proposals_path(cfg);
    let raw = fs::read_to_string(&path)
        .map_err(|_| format!("no saved proposals ({}) — run `ralph learn` first", path.display()))?;
    parse_proposals(&raw).map_err(|e| e.into())
}

/// Parse an `--apply` selection like `1,3` into 0-based indices.
pub fn parse_selection(spec: &str, len: usize) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for part in spec.split(',') {
        let n: usize = part
            .trim()
            .parse()
            .map_err(|_| format!("invalid selection '{part}'"))?;
        if n == 0 || n > len {
            return Err(format!("selection {n} out of range (1..={len})"));
        }
        out.push(n - 1);
    }
    Ok(out)
}

/// `ralph learn [--apply [ids]] [--discard]`.
pub fn run(args: &[String]) -> R<i32> {
    let cfg = crate::config::load_base(args)?;
    let dir = learnings_dir(&cfg);

    if args.iter().any(|a| a == "--discard") {
        let _ = fs::remove_file(proposals_path(&cfg));
        println!("proposals discarded");
        return Ok(0);
    }

    if let Some(pos) = args.iter().position(|a| a == "--apply") {
        let proposals = load_proposals(&cfg)?;
        if proposals.is_empty() {
            println!("no proposals to apply");
            return Ok(0);
        }
        // An optional non-flag value after --apply selects a subset.
        let indices = match args.get(pos + 1).filter(|v| !v.starts_with("--")) {
            Some(spec) => parse_selection(spec, proposals.len())?,
            None => (0..proposals.len()).collect(),
        };
        for i in indices {
            let path = write_learning(&dir, &proposals[i])?;
            println!("wrote {}", path.display());
        }
        let _ = fs::remove_file(proposals_path(&cfg));
        return Ok(0);
    }

    // Mine.
    let run_log = read_tail(&cfg.dir.join("run.log"), RUN_LOG_TAIL_BYTES);
    if run_log.trim().is_empty() {
        println!("nothing to mine: {} is empty or absent", cfg.dir.join("run.log").display());
        return Ok(1);
    }
    let carry = fs::read_to_string(&cfg.progress).unwrap_or_default();
    let existing = existing_titles(&dir);
    let prompt = build_mine_prompt(&run_log, &carry, &existing);
    eprintln!("mining run.log with {} …", cfg.synth_model);
    let raw = synth::run_claude_oneshot(&cfg.synth_model, LEARN_TIMEOUT_SECS, &prompt)
        .ok_or("miner call failed or timed out")?;
    let proposals = match parse_proposals(&raw) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}; raw output follows:\n{raw}");
            return Ok(1);
        }
    };
    if proposals.is_empty() {
        println!("no non-obvious learnings found — nothing proposed");
        let _ = fs::remove_file(proposals_path(&cfg));
        return Ok(0);
    }
    save_proposals(&cfg, &proposals)?;
    println!("proposed {} learning(s):\n", proposals.len());
    for (i, l) in proposals.iter().enumerate() {
        println!("{}. {} ({}.md)\n   {}\n", i + 1, l.title, l.slug, l.body.replace('\n', "\n   "));
    }
    println!(
        "apply with `ralph learn --apply` (all) or `ralph learn --apply 1,3` (subset); `ralph learn --discard` drops them"
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmp() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "ralph-learn-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn slug_sanitization() {
        assert_eq!(sanitize_slug("Godot OOM! lesson"), "godot-oom-lesson");
        assert_eq!(sanitize_slug("--weird--"), "weird");
        assert_eq!(sanitize_slug("!!!"), "learning");
    }

    #[test]
    fn parse_accepts_plain_and_fenced_json_drops_invalid_entries() {
        let plain = r#"[{"slug":"a-b","title":"T","body":"B"},{"title":"missing slug","body":"x"}]"#;
        let got = parse_proposals(plain).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].slug, "a-b");

        let fenced = "```json\n[{\"slug\":\"c\",\"title\":\"T2\",\"body\":\"B2\"}]\n```";
        assert_eq!(parse_proposals(fenced).unwrap().len(), 1);

        assert_eq!(parse_proposals("[]").unwrap(), Vec::new());
        assert!(parse_proposals("not json").is_err());
        assert!(parse_proposals("{\"an\":\"object\"}").is_err());
    }

    #[test]
    fn mine_prompt_carries_material_and_discipline() {
        let p = build_mine_prompt("iter 3 ok", "- watch foo", &["Old lesson".into()]);
        assert!(p.contains("iter 3 ok"));
        assert!(p.contains("watch foo"));
        assert!(p.contains("Old lesson"));
        assert!(p.contains("output []"));
    }

    #[test]
    fn write_learning_suffixes_taken_slugs_and_titles_dedupe() {
        let dir = tmp();
        let l = Learning {
            slug: "godot-oom".into(),
            title: "Godot leaks under headless".into(),
            body: "Kill it.".into(),
        };
        let p1 = write_learning(&dir, &l).unwrap();
        let p2 = write_learning(&dir, &l).unwrap();
        assert!(p1.ends_with("godot-oom.md"));
        assert!(p2.ends_with("godot-oom-2.md"));
        let titles = existing_titles(&dir);
        assert_eq!(titles.len(), 2);
        assert_eq!(titles[0], "Godot leaks under headless");
    }

    #[test]
    fn injection_block_is_bounded_and_reports_omissions() {
        let dir = tmp();
        assert_eq!(injection_block(&dir), ""); // no dir/files → nothing
        for i in 0..12 {
            write_learning(
                &dir,
                &Learning {
                    slug: format!("l{i:02}"),
                    title: format!("Lesson {i}"),
                    body: "x".repeat(500),
                },
            )
            .unwrap();
        }
        let block = injection_block(&dir);
        assert!(block.contains("## Learnings"));
        assert!(block.len() < INJECT_TOTAL_BYTES + 400, "was {}", block.len());
        assert!(block.contains("omitted"), "over-budget files must be named");
    }

    #[test]
    fn selection_parsing() {
        assert_eq!(parse_selection("1,3", 4).unwrap(), vec![0, 2]);
        assert!(parse_selection("0", 4).is_err());
        assert!(parse_selection("5", 4).is_err());
        assert!(parse_selection("x", 4).is_err());
    }
}
