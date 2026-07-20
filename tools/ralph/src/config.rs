//! Configuration: defaults ← `.ralph/ralph.toml` ← env (`RALPH_*`) ← flags.
//!
//! The file is optional (absent → all defaults). Env and flags override it in
//! that order. Env access and argv are injected into the merge helpers so the
//! precedence rules can be unit-tested without touching the real process
//! environment.

use serde::Deserialize;
use std::path::PathBuf;

/// Fully-resolved runtime configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub model: String,
    pub fallback_model: String,
    pub max_iterations: u64,
    pub marker: String,
    pub prompt: PathBuf,
    pub dir: PathBuf,
    /// Backlog file to archive on completion.
    pub backlog: PathBuf,
    /// Durable current hand-off / progress log.
    pub progress: PathBuf,
    /// Claude effort: `auto` maps model tiers, `inherit` defers to settings.
    pub effort: String,
    pub yolo: bool,
    pub output_format: String,
    pub limit_wait: u64,
    pub limit_wait_max: u64,
    pub transient_wait: u64,
    pub transient_wait_max: u64,
    pub extra_args: Vec<String>,
    /// Cumulative cost cap in USD; 0 = off.
    pub max_cost_usd: f64,
    /// Wall-clock cap in seconds; 0 = off.
    pub max_duration: u64,
    /// Per-iteration timeout in seconds; 0 = off.
    pub iteration_timeout: u64,
    /// No-progress streak length that triggers a model escalation.
    pub escalate_after: u32,
    /// No-progress streak length that aborts the loop.
    pub abort_after: u32,
    /// Model tiers, ascending, used when escalating on no-progress.
    pub escalation_ladder: Vec<String>,
    /// Single iteration then exit (testing); flag-only.
    pub once: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            model: "sonnet".into(),
            fallback_model: "sonnet".into(),
            max_iterations: 0,
            marker: "RALPH_COMPLETE".into(),
            prompt: PathBuf::from(".ralph/PROMPT.md"),
            dir: PathBuf::from(".ralph"),
            backlog: PathBuf::from(".ralph/BACKLOG.md"),
            progress: PathBuf::from(".ralph/PROGRESS.md"),
            effort: "auto".into(),
            yolo: true,
            output_format: "stream-json".into(),
            limit_wait: 300,
            limit_wait_max: 3600,
            transient_wait: 10,
            transient_wait_max: 300,
            extra_args: Vec::new(),
            max_cost_usd: 0.0,
            max_duration: 0,
            iteration_timeout: 0,
            escalate_after: 2,
            abort_after: 4,
            escalation_ladder: vec!["haiku".into(), "sonnet".into(), "opus".into()],
            once: false,
        }
    }
}

/// The subset of config expressible in `ralph.toml`. All optional; `None` fields
/// leave the running default untouched.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileConfig {
    pub model: Option<String>,
    pub fallback_model: Option<String>,
    pub max_iterations: Option<u64>,
    pub marker: Option<String>,
    pub prompt: Option<String>,
    pub dir: Option<String>,
    pub backlog: Option<String>,
    pub progress: Option<String>,
    pub effort: Option<String>,
    pub yolo: Option<bool>,
    pub output_format: Option<String>,
    pub limit_wait: Option<u64>,
    pub limit_wait_max: Option<u64>,
    pub transient_wait: Option<u64>,
    pub transient_wait_max: Option<u64>,
    pub extra_args: Option<ExtraArgs>,
    pub max_cost_usd: Option<f64>,
    /// Accepts a bare number of seconds or a suffixed string (`8h`, `30m`).
    pub max_duration: Option<DurationSpec>,
    pub iteration_timeout: Option<DurationSpec>,
    pub escalate_after: Option<u32>,
    pub abort_after: Option<u32>,
    pub escalation_ladder: Option<Vec<String>>,
}

/// `extra_args` may be a single string ("--foo --bar") or an array of strings.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ExtraArgs {
    List(Vec<String>),
    Line(String),
}

impl ExtraArgs {
    fn into_vec(self) -> Vec<String> {
        match self {
            ExtraArgs::List(v) => v,
            ExtraArgs::Line(s) => split_args(&s),
        }
    }
}

/// A duration accepted from TOML as either an integer (seconds) or a suffixed
/// string (`300s`, `30m`, `8h`, `1d`).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum DurationSpec {
    Secs(u64),
    Text(String),
}

impl DurationSpec {
    fn resolve(self) -> Result<u64, String> {
        match self {
            DurationSpec::Secs(n) => Ok(n),
            DurationSpec::Text(s) => parse_duration(&s),
        }
    }
}

/// Parse a duration into seconds. Bare number = seconds; suffix `s`/`m`/`h`/`d`.
pub fn parse_duration(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration".into());
    }
    let (num, mult) = match s.chars().last().unwrap() {
        's' => (&s[..s.len() - 1], 1),
        'm' => (&s[..s.len() - 1], 60),
        'h' => (&s[..s.len() - 1], 3600),
        'd' => (&s[..s.len() - 1], 86_400),
        c if c.is_ascii_digit() => (s, 1),
        c => return Err(format!("invalid duration suffix '{c}' in '{s}'")),
    };
    let n: u64 = num
        .trim()
        .parse()
        .map_err(|_| format!("invalid duration number in '{s}'"))?;
    Ok(n * mult)
}

/// Split a shell-ish argument line on whitespace (no quote handling — matches
/// the bash `$RALPH_EXTRA_ARGS` word-splitting behavior).
fn split_args(s: &str) -> Vec<String> {
    s.split_whitespace().map(String::from).collect()
}

/// Apply file-config Options over a Config.
pub fn apply_file(cfg: &mut Config, f: FileConfig) -> Result<(), String> {
    if let Some(v) = f.model {
        cfg.model = v;
    }
    if let Some(v) = f.fallback_model {
        cfg.fallback_model = v;
    }
    if let Some(v) = f.max_iterations {
        cfg.max_iterations = v;
    }
    if let Some(v) = f.marker {
        cfg.marker = v;
    }
    if let Some(v) = f.prompt {
        cfg.prompt = PathBuf::from(v);
    }
    if let Some(v) = f.dir {
        cfg.dir = PathBuf::from(v);
    }
    if let Some(v) = f.backlog {
        cfg.backlog = PathBuf::from(v);
    }
    if let Some(v) = f.progress {
        cfg.progress = PathBuf::from(v);
    }
    if let Some(v) = f.effort {
        cfg.effort = v;
    }
    if let Some(v) = f.yolo {
        cfg.yolo = v;
    }
    if let Some(v) = f.output_format {
        cfg.output_format = v;
    }
    if let Some(v) = f.limit_wait {
        cfg.limit_wait = v;
    }
    if let Some(v) = f.limit_wait_max {
        cfg.limit_wait_max = v;
    }
    if let Some(v) = f.transient_wait {
        cfg.transient_wait = v;
    }
    if let Some(v) = f.transient_wait_max {
        cfg.transient_wait_max = v;
    }
    if let Some(v) = f.extra_args {
        cfg.extra_args = v.into_vec();
    }
    if let Some(v) = f.max_cost_usd {
        cfg.max_cost_usd = v;
    }
    if let Some(v) = f.max_duration {
        cfg.max_duration = v.resolve()?;
    }
    if let Some(v) = f.iteration_timeout {
        cfg.iteration_timeout = v.resolve()?;
    }
    if let Some(v) = f.escalate_after {
        cfg.escalate_after = v;
    }
    if let Some(v) = f.abort_after {
        cfg.abort_after = v;
    }
    if let Some(v) = f.escalation_ladder {
        cfg.escalation_ladder = v;
    }
    Ok(())
}

/// Apply `RALPH_*` env overrides. `get` maps a var name to its value (injected
/// for testability).
pub fn apply_env<F: Fn(&str) -> Option<String>>(cfg: &mut Config, get: F) -> Result<(), String> {
    macro_rules! set_str {
        ($var:literal, $field:expr) => {
            if let Some(v) = get($var) {
                $field = v;
            }
        };
    }
    macro_rules! set_parse {
        ($var:literal, $field:expr, $ty:ty) => {
            if let Some(v) = get($var) {
                $field = v
                    .trim()
                    .parse::<$ty>()
                    .map_err(|_| format!("invalid {}: '{}'", $var, v))?;
            }
        };
    }
    set_str!("RALPH_MODEL", cfg.model);
    set_str!("RALPH_FALLBACK_MODEL", cfg.fallback_model);
    set_parse!("RALPH_MAX_ITER", cfg.max_iterations, u64);
    set_str!("RALPH_MARKER", cfg.marker);
    if let Some(v) = get("RALPH_PROMPT") {
        cfg.prompt = PathBuf::from(v);
    }
    if let Some(v) = get("RALPH_DIR") {
        cfg.dir = PathBuf::from(v);
    }
    if let Some(v) = get("RALPH_BACKLOG") {
        cfg.backlog = PathBuf::from(v);
    }
    if let Some(v) = get("RALPH_PROGRESS") {
        cfg.progress = PathBuf::from(v);
    }
    set_str!("RALPH_EFFORT", cfg.effort);
    if let Some(v) = get("RALPH_YOLO") {
        cfg.yolo = v != "0" && !v.eq_ignore_ascii_case("false");
    }
    set_str!("RALPH_OUTPUT_FORMAT", cfg.output_format);
    set_parse!("RALPH_LIMIT_WAIT", cfg.limit_wait, u64);
    set_parse!("RALPH_LIMIT_WAIT_MAX", cfg.limit_wait_max, u64);
    set_parse!("RALPH_TRANSIENT_WAIT", cfg.transient_wait, u64);
    set_parse!("RALPH_TRANSIENT_WAIT_MAX", cfg.transient_wait_max, u64);
    if let Some(v) = get("RALPH_EXTRA_ARGS") {
        cfg.extra_args = split_args(&v);
    }
    set_parse!("RALPH_MAX_COST", cfg.max_cost_usd, f64);
    if let Some(v) = get("RALPH_MAX_DURATION") {
        cfg.max_duration = parse_duration(&v)?;
    }
    if let Some(v) = get("RALPH_ITER_TIMEOUT") {
        cfg.iteration_timeout = parse_duration(&v)?;
    }
    set_parse!("RALPH_ESCALATE_AFTER", cfg.escalate_after, u32);
    set_parse!("RALPH_ABORT_AFTER", cfg.abort_after, u32);
    Ok(())
}

/// Apply command-line flags (highest precedence). Returns `Ok(true)` if a
/// help flag was seen (caller should print usage and exit 0).
pub fn apply_args(cfg: &mut Config, args: &[String]) -> Result<bool, String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut next = || it.next().cloned().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--prompt" => cfg.prompt = PathBuf::from(next()?),
            "--model" => cfg.model = next()?,
            "--fallback-model" => cfg.fallback_model = next()?,
            "--max-iterations" => {
                cfg.max_iterations = next()?.parse().map_err(|_| "bad --max-iterations")?
            }
            "--marker" => cfg.marker = next()?,
            "--dir" => cfg.dir = PathBuf::from(next()?),
            "--backlog" => cfg.backlog = PathBuf::from(next()?),
            "--progress" => cfg.progress = PathBuf::from(next()?),
            "--effort" => cfg.effort = next()?,
            "--max-cost" => cfg.max_cost_usd = next()?.parse().map_err(|_| "bad --max-cost")?,
            "--max-duration" => cfg.max_duration = parse_duration(&next()?)?,
            "--iteration-timeout" => cfg.iteration_timeout = parse_duration(&next()?)?,
            "--escalate-after" => {
                cfg.escalate_after = next()?.parse().map_err(|_| "bad --escalate-after")?
            }
            "--abort-after" => {
                cfg.abort_after = next()?.parse().map_err(|_| "bad --abort-after")?
            }
            "--once" => cfg.once = true,
            "--no-yolo" => cfg.yolo = false,
            // --config is consumed earlier (see config_path); skip its value here.
            "--config" => {
                let _ = next()?;
            }
            "-h" | "--help" => return Ok(true),
            other => return Err(format!("unknown arg: {other}")),
        }
    }
    Ok(false)
}

/// Resolve the config-file path from argv/env before the full merge (so the
/// file can be loaded first and then overridden). Default
/// `.ralph/ralph.toml`.
pub fn config_path<F: Fn(&str) -> Option<String>>(args: &[String], get: F) -> PathBuf {
    // Flag wins over env.
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--config" {
            if let Some(p) = args.get(i + 1) {
                return PathBuf::from(p);
            }
        }
        i += 1;
    }
    if let Some(p) = get("RALPH_CONFIG") {
        return PathBuf::from(p);
    }
    PathBuf::from(".ralph/ralph.toml")
}

/// Validate cross-field invariants after the full merge.
pub fn validate(cfg: &Config) -> Result<(), String> {
    if cfg.abort_after < cfg.escalate_after {
        return Err(format!(
            "abort_after ({}) must be >= escalate_after ({})",
            cfg.abort_after, cfg.escalate_after
        ));
    }
    if cfg.escalation_ladder.is_empty() {
        return Err("escalation_ladder must not be empty".into());
    }
    if !matches!(
        cfg.effort.as_str(),
        "auto" | "inherit" | "low" | "medium" | "high" | "xhigh" | "max"
    ) {
        return Err(format!(
            "invalid effort '{}': expected auto, inherit, low, medium, high, xhigh, or max",
            cfg.effort
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_suffixes() {
        assert_eq!(parse_duration("300"), Ok(300));
        assert_eq!(parse_duration("300s"), Ok(300));
        assert_eq!(parse_duration("30m"), Ok(1800));
        assert_eq!(parse_duration("8h"), Ok(28_800));
        assert_eq!(parse_duration("1d"), Ok(86_400));
        assert!(parse_duration("8x").is_err());
        assert!(parse_duration("").is_err());
        assert!(parse_duration("abc").is_err());
    }

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert_eq!(c.model, "sonnet");
        assert_eq!(c.escalate_after, 2);
        assert_eq!(c.abort_after, 4);
        assert!(c.yolo);
        assert_eq!(c.escalation_ladder, vec!["haiku", "sonnet", "opus"]);
        assert_eq!(c.prompt, PathBuf::from(".ralph/PROMPT.md"));
        assert_eq!(c.progress, PathBuf::from(".ralph/PROGRESS.md"));
        assert_eq!(c.effort, "auto");
    }

    #[test]
    fn file_overrides_defaults() {
        let toml = r#"
            model = "opus"
            max_cost_usd = 12.5
            max_duration = "8h"
            iteration_timeout = 600
            progress = "notes/NOW.md"
            effort = "medium"
            extra_args = ["--add-dir", "/tmp"]
            escalate_after = 3
            abort_after = 5
        "#;
        let f: FileConfig = toml::from_str(toml).unwrap();
        let mut c = Config::default();
        apply_file(&mut c, f).unwrap();
        assert_eq!(c.model, "opus");
        assert_eq!(c.max_cost_usd, 12.5);
        assert_eq!(c.max_duration, 28_800);
        assert_eq!(c.iteration_timeout, 600);
        assert_eq!(c.progress, PathBuf::from("notes/NOW.md"));
        assert_eq!(c.effort, "medium");
        assert_eq!(c.extra_args, vec!["--add-dir", "/tmp"]);
        assert_eq!(c.escalate_after, 3);
        assert_eq!(c.abort_after, 5);
    }

    #[test]
    fn extra_args_as_line() {
        let f: FileConfig = toml::from_str(r#"extra_args = "--foo --bar baz""#).unwrap();
        let mut c = Config::default();
        apply_file(&mut c, f).unwrap();
        assert_eq!(c.extra_args, vec!["--foo", "--bar", "baz"]);
    }

    #[test]
    fn unknown_toml_key_rejected() {
        assert!(toml::from_str::<FileConfig>("nonsense = 1").is_err());
    }

    #[test]
    fn env_overrides_file() {
        let mut c = Config::default();
        apply_file(&mut c, toml::from_str(r#"model = "opus""#).unwrap()).unwrap();
        let env = |k: &str| match k {
            "RALPH_MODEL" => Some("haiku".to_string()),
            "RALPH_MAX_COST" => Some("5".to_string()),
            "RALPH_YOLO" => Some("0".to_string()),
            "RALPH_ITER_TIMEOUT" => Some("10m".to_string()),
            _ => None,
        };
        apply_env(&mut c, env).unwrap();
        assert_eq!(c.model, "haiku"); // env beat file
        assert_eq!(c.max_cost_usd, 5.0);
        assert!(!c.yolo);
        assert_eq!(c.iteration_timeout, 600);
    }

    #[test]
    fn flags_override_env() {
        let mut c = Config::default();
        apply_env(&mut c, |k| (k == "RALPH_MODEL").then(|| "haiku".to_string())).unwrap();
        let args: Vec<String> = ["--model", "opus", "--max-cost", "20", "--no-yolo", "--once"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let help = apply_args(&mut c, &args).unwrap();
        assert!(!help);
        assert_eq!(c.model, "opus"); // flag beat env
        assert_eq!(c.max_cost_usd, 20.0);
        assert!(!c.yolo);
        assert!(c.once);
    }

    #[test]
    fn bad_env_number_errors() {
        let mut c = Config::default();
        let r = apply_env(&mut c, |k| (k == "RALPH_MAX_ITER").then(|| "lots".to_string()));
        assert!(r.is_err());
    }

    #[test]
    fn help_flag_detected() {
        let mut c = Config::default();
        assert!(apply_args(&mut c, &["--help".to_string()]).unwrap());
    }

    #[test]
    fn config_path_precedence() {
        let args = vec!["--config".to_string(), "/a/b.toml".to_string()];
        assert_eq!(config_path(&args, |_| None), PathBuf::from("/a/b.toml"));
        assert_eq!(
            config_path(&[], |k| (k == "RALPH_CONFIG").then(|| "/env.toml".to_string())),
            PathBuf::from("/env.toml")
        );
        assert_eq!(config_path(&[], |_| None), PathBuf::from(".ralph/ralph.toml"));
    }

    #[test]
    fn validate_rejects_bad_thresholds() {
        let mut c = Config { escalate_after: 5, abort_after: 2, ..Config::default() };
        assert!(validate(&c).is_err());
        c.abort_after = 6;
        assert!(validate(&c).is_ok());
    }

    #[test]
    fn backlog_precedence() {
        let mut c = Config::default();
        assert_eq!(c.backlog, PathBuf::from(".ralph/BACKLOG.md"));
        apply_file(&mut c, toml::from_str(r#"backlog = "a/B.md""#).unwrap()).unwrap();
        assert_eq!(c.backlog, PathBuf::from("a/B.md"));
        apply_env(&mut c, |k| (k == "RALPH_BACKLOG").then(|| "b/B.md".to_string())).unwrap();
        assert_eq!(c.backlog, PathBuf::from("b/B.md"));
        apply_args(&mut c, &["--backlog".into(), "c/B.md".into()]).unwrap();
        assert_eq!(c.backlog, PathBuf::from("c/B.md"));
    }

    #[test]
    fn progress_and_effort_precedence() {
        let mut c = Config::default();
        let file: FileConfig = toml::from_str(
            "progress = \"a/P.md\"\neffort = \"low\"\n",
        )
        .unwrap();
        apply_file(&mut c, file).unwrap();
        assert_eq!(c.progress, PathBuf::from("a/P.md"));
        assert_eq!(c.effort, "low");

        apply_env(&mut c, |k| match k {
            "RALPH_PROGRESS" => Some("b/P.md".into()),
            "RALPH_EFFORT" => Some("medium".into()),
            _ => None,
        })
        .unwrap();
        apply_args(
            &mut c,
            &[
                "--progress".into(),
                "c/P.md".into(),
                "--effort".into(),
                "high".into(),
            ],
        )
        .unwrap();
        assert_eq!(c.progress, PathBuf::from("c/P.md"));
        assert_eq!(c.effort, "high");
    }

    #[test]
    fn validate_rejects_unknown_effort() {
        let c = Config { effort: "heroic".into(), ..Config::default() };
        assert!(validate(&c).is_err());
    }
}
