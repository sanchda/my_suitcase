//! Read-only setup checks. Never invokes a model or executes verification commands.
use crate::{backend, config, context, R};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub fn executable(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let usable = |p: &Path| {
        p.is_file()
            && p.metadata()
                .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    };
    if name.contains('/') {
        return usable(Path::new(name)).then(|| PathBuf::from(name));
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|p| p.join(name))
        .find(|p| usable(p))
}

#[derive(Serialize)]
struct Check {
    name: String,
    status: String,
    detail: String,
}
#[derive(Default, Serialize)]
struct Report {
    passed: bool,
    checks: Vec<Check>,
}
impl Report {
    fn add(&mut self, name: &str, status: &str, detail: impl Into<String>) {
        self.checks.push(Check {
            name: name.into(),
            status: status.into(),
            detail: detail.into(),
        });
    }
    fn inspect(&mut self, cfg: &config::Config) {
        let cwd = std::env::current_dir().unwrap_or_default();
        self.add("worktree", "pass", cwd.display().to_string());
        self.add("runtime", "pass", cwd.join(&cfg.dir).display().to_string());
        match crate::git::branch(Path::new(".")) {
            Some(branch) => self.add("branch", "pass", branch),
            None => self.add(
                "branch",
                "warn",
                "no named Git branch; Git contract checks are limited",
            ),
        }
        let root = std::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok();
        if let Some(output) = root.filter(|o| o.status.success()) {
            let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if std::fs::canonicalize(&root).ok() != std::fs::canonicalize(&cwd).ok() {
                self.add(
                    "worktree-root",
                    "warn",
                    format!("launch from {root}; Ralph uses the current directory"),
                );
            }
        }
        let resolved = context::load(&cfg.backlog, &cfg.progress);
        self.add(
            "backlog",
            if resolved.has_errors() {
                "fail"
            } else {
                "pass"
            },
            resolved.lint_report(),
        );
        match context::full_prompt(cfg, &resolved) {
            Ok(prompt) => {
                // The scaffold explains placeholder syntax in an HTML comment.
                let visible = regex::Regex::new(r"<!--[\s\S]*?-->")
                    .unwrap()
                    .replace_all(&prompt, "");
                let unresolved = regex::Regex::new(r"\{\{[\s\S]*?\}\}")
                    .unwrap()
                    .find_iter(&visible)
                    .count();
                self.add("prompt", if unresolved > 0 { "fail" } else { "pass" }, format!("{} bytes; {unresolved} unfilled placeholders; preview with ralph brief --full", prompt.len()));
                let progress = std::fs::read_to_string(&cfg.progress).unwrap_or_default();
                if progress.trim().len() > crate::synth::MAX_CARRY_FORWARD_BYTES {
                    self.add(
                        "carry-forward",
                        "warn",
                        "progress exceeds 1200 bytes and will be truncated in the prompt",
                    );
                }
            }
            Err(e) => self.add("prompt", "fail", e.to_string()),
        }
        let requested = crate::control::preview_model(cfg, &resolved);
        let selection = backend::resolve(cfg, &requested);
        self.add(
            "model",
            "pass",
            format!(
                "{} / {} / effort {}",
                selection.backend.executable(),
                selection.model.as_deref().unwrap_or("configured default"),
                selection.effort.as_deref().unwrap_or("inherited")
            ),
        );
        for (role, model) in [
            ("worker", requested.as_str()),
            ("synth", cfg.synth_model.as_str()),
            ("judge", cfg.judge_model.as_str()),
        ] {
            if role == "judge"
                && cfg.judge_tiers.is_empty()
                && cfg
                    .acceptance
                    .values()
                    .all(|p| p.review == crate::acceptance::Review::Off)
            {
                continue;
            }
            let s = backend::resolve(cfg, model);
            self.add(
                role,
                if executable(s.backend.executable()).is_some() {
                    "pass"
                } else {
                    "fail"
                },
                format!(
                    "{} executable (authentication is not probed)",
                    s.backend.executable()
                ),
            );
            if let Err(e) = backend::check_cost_budget(cfg, &s) {
                self.add("budget", "fail", e.to_string());
            }
        }
        self.add("permissions", "pass", if cfg.yolo { "backend permission bypass enabled" } else { "backend normal permissions; Codex uses workspace-write without interactive approvals" });
        self.add("budgets", "pass", format!("iterations={}, worker USD={}, persisted USD={}, boundary duration={}s, worker timeout={}s", cfg.max_iterations, cfg.max_cost_usd, cfg.budget_usd, cfg.max_duration, cfg.iteration_timeout));
        let doc = crate::backlog::Document::parse(
            &std::fs::read_to_string(&cfg.backlog).unwrap_or_default(),
        );
        for (task, policy) in &cfg.acceptance {
            if task != "@complete" && !doc.tasks.iter().any(|t| &t.id == task) {
                self.add(
                    "acceptance-task",
                    "warn",
                    format!("{task}: no matching task in the live backlog"),
                );
            }
            if let Some(program) = policy.command.first() {
                self.add(
                    "verification",
                    if executable(program).is_some() {
                        "pass"
                    } else {
                        "fail"
                    },
                    format!(
                        "{task}: {:?}, timeout {}s (not executed)",
                        policy.command, policy.timeout_secs
                    ),
                );
            }
        }
    }
}

pub fn run(args: &[String]) -> R<i32> {
    let json = args.iter().any(|a| a == "--json");
    let rest = args
        .iter()
        .filter(|a| a.as_str() != "--json")
        .cloned()
        .collect::<Vec<_>>();
    let mut report = Report::default();
    let loaded = config::load_base(&rest).and_then(|mut cfg| {
        if config::apply_args(&mut cfg, &rest)? {
            println!("Usage: ralph doctor [--json] [--dir <path>] [--config <file>]");
            return Ok(None);
        }
        config::validate(&cfg)?;
        Ok(Some(cfg))
    });
    match loaded {
        Ok(Some(cfg)) => report.inspect(&cfg),
        Ok(None) => return Ok(0),
        Err(e) => report.add("configuration", "fail", e.to_string()),
    }
    report.passed = report.checks.iter().all(|c| c.status != "fail");
    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        for c in &report.checks {
            println!("{} {}: {}", c.status, c.name, c.detail.trim());
        }
    }
    Ok(if report.passed { 0 } else { 1 })
}
