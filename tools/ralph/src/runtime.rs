//! Small durable records and snapshots; no model calls or product mutations.
use crate::{config::Config, git, R};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub fn write_json(path: &Path, value: &impl Serialize) -> R<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::backlog_edit::write_atomic(path, &serde_json::to_string_pretty(value)?)
}

/// Stable across processes and releases (FNV-1a); identifies content, not security.
pub fn fingerprint(text: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for b in text.bytes() {
        hash = (hash ^ u64::from(b)).wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub fn bounded(text: &str, bytes: usize) -> &str {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Record {
    pub run_id: String,
    pub pid: u32,
    pub started_at: i64,
    pub updated_at: i64,
    pub worktree: PathBuf,
    pub branch: Option<String>,
    pub phase: String,
    pub terminal_reason: Option<String>,
    pub attempt: u64,
    pub iteration: u64,
    pub task: Option<String>,
    pub model: Option<String>,
    pub backend: Option<String>,
    pub last_accepted_revision: Option<String>,
    pub worker_cost_usd: f64,
    pub cost_unreported: bool,
    pub task_attempts: u32,
    pub retry_at: Option<i64>,
    pub artifacts: Option<PathBuf>,
}

pub struct Run {
    pub record: Record,
    base: PathBuf,
    home: PathBuf,
}

impl Run {
    pub fn start(cfg: &Config) -> R<Self> {
        let now = chrono::Utc::now();
        let id = format!(
            "{}-{}",
            now.format("%Y%m%dT%H%M%S%.9fZ"),
            std::process::id()
        );
        let home = cfg.dir.join("runs").join(&id);
        std::fs::create_dir_all(&home)?;
        let run = Self {
            record: Record {
                run_id: id,
                pid: std::process::id(),
                started_at: now.timestamp(),
                updated_at: now.timestamp(),
                worktree: std::env::current_dir()?,
                branch: git::branch(Path::new(".")),
                phase: "starting".into(),
                terminal_reason: None,
                attempt: 0,
                iteration: 0,
                task: None,
                model: None,
                backend: None,
                last_accepted_revision: None,
                worker_cost_usd: 0.0,
                cost_unreported: false,
                task_attempts: 0,
                retry_at: None,
                artifacts: None,
            },
            base: cfg.dir.clone(),
            home,
        };
        run.save()?;
        Ok(run)
    }

    pub fn save(&self) -> R<()> {
        write_json(&self.home.join("run.json"), &self.record)?;
        write_json(&self.base.join("run.json"), &self.record)
    }

    pub fn phase(&mut self, phase: &str) -> R<()> {
        self.record.phase = phase.into();
        self.record.updated_at = chrono::Utc::now().timestamp();
        self.save()
    }

    pub fn finish(&mut self, reason: &str) -> R<()> {
        self.record.terminal_reason = Some(reason.into());
        self.phase("stopped")
    }

    pub fn begin_attempt(
        &mut self,
        cfg: &Config,
        iteration: u64,
        task: Option<&str>,
        model: &str,
        backend: &str,
        prompt: &str,
    ) -> R<PathBuf> {
        self.record.attempt += 1;
        self.record.iteration = iteration;
        self.record.task = task.map(str::to_string);
        self.record.model = Some(model.into());
        self.record.backend = Some(backend.into());
        self.record.retry_at = None;
        let path = self
            .home
            .join(format!("attempt-{:04}", self.record.attempt));
        std::fs::create_dir_all(&path)?;
        std::fs::write(path.join("prompt.md"), prompt)?;
        let mut config = serde_json::to_value(cfg)?;
        config.as_object_mut().unwrap().remove("discord_webhook");
        write_json(&path.join("config.json"), &config)?;
        for (name, source) in [("BACKLOG.md", &cfg.backlog), ("PROGRESS.md", &cfg.progress)] {
            if source.exists() {
                std::fs::copy(source, path.join(name))?;
            }
        }
        write_json(
            &path.join("git.json"),
            &serde_json::json!({"head": git::head(Path::new(".")), "branch": git::branch(Path::new("."))}),
        )?;
        self.record.artifacts = Some(path.clone());
        self.phase("working")?;
        Ok(path)
    }
}

/// Read-only status: a dead process cannot leave a permanently 'working' card.
pub fn read(base: &Path) -> Option<Record> {
    let mut r: Record =
        serde_json::from_str(&std::fs::read_to_string(base.join("run.json")).ok()?).ok()?;
    if r.terminal_reason.is_none() && !crate::pidguard::is_alive(r.pid) {
        r.phase = "interrupted".into();
        r.terminal_reason = Some(
            std::fs::read_to_string(base.join("runs").join(&r.run_id).join("stop-request.txt"))
                .unwrap_or_else(|_| "process exited without a terminal record".into()),
        );
    }
    Some(r)
}

/// A separate file avoids racing the loop's atomic run-record updates.
pub fn note_stop(base: &Path, force: bool) -> R<()> {
    if let Some(record) = read(base).filter(|r| r.terminal_reason.is_none()) {
        let path = base
            .join("runs")
            .join(record.run_id)
            .join("stop-request.txt");
        crate::backlog_edit::write_atomic(
            &path,
            if force {
                "forced stop requested"
            } else {
                "stop requested"
            },
        )?;
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Feedback {
    pub task: String,
    pub contract: String,
    pub reason: String,
    pub revision: Option<String>,
    pub evidence: PathBuf,
}

pub fn feedback(base: &Path, task: &str, contract: &str, reason: &str, evidence: &Path) -> R<()> {
    write_json(
        &base.join("previous-attempt.json"),
        &Feedback {
            task: task.into(),
            contract: fingerprint(contract),
            reason: bounded(reason, 4096).into(),
            revision: git::head(Path::new(".")),
            evidence: evidence.to_path_buf(),
        },
    )
}

pub fn feedback_prompt(base: &Path, task: &str, contract: &str) -> String {
    let feedback = std::fs::read_to_string(base.join("previous-attempt.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Feedback>(&s).ok());
    match feedback {
        Some(f) if f.task == task && f.contract == fingerprint(contract) => format!(
            "\n## Previous attempt — runner-observed feedback\n{}\nRevision: {}\nFull evidence: {}\n",
            bounded(&f.reason, 4096), f.revision.as_deref().unwrap_or("unknown"), f.evidence.display()),
        _ => String::new(),
    }
}
