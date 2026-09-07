//! Explicit, task-local acceptance gates. Prose Verify contracts remain prose.
use crate::{config::Config, judge, runtime, R};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Review {
    #[default]
    Off,
    Advisory,
    Required,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub command: Vec<String>,
    pub timeout_secs: u64,
    pub review: Review,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            command: vec![],
            timeout_secs: 120,
            review: Review::Off,
        }
    }
}
impl Policy {
    pub fn validate(&self) -> Result<(), String> {
        if self.timeout_secs == 0 {
            return Err("timeout_secs must be greater than zero".into());
        }
        if self.command.first().is_some_and(|c| c.trim().is_empty()) {
            return Err("command needs a nonempty executable".into());
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub command: Vec<String>,
    pub revision: Option<String>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub error: Option<String>,
}
impl Receipt {
    pub fn passed(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && self.error.is_none()
    }
}

pub fn check(policy: &Policy, task: &str, evidence: &Path) -> R<Receipt> {
    let mut result = Receipt {
        command: policy.command.clone(),
        revision: crate::git::head(Path::new(".")),
        exit_code: None,
        timed_out: false,
        error: None,
    };
    let stdout = std::fs::File::create(evidence.join("verify.stdout"))?;
    let stderr = std::fs::File::create(evidence.join("verify.stderr"))?;
    let mut cmd = Command::new(&policy.command[0]);
    cmd.args(&policy.command[1..])
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .env("RALPH_TASK_ID", task);
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
    match cmd.spawn() {
        Err(e) => result.error = Some(format!("cannot launch verification: {e}")),
        Ok(mut child) => {
            let _active = crate::control::ActiveProcess::new(child.id());
            let start = Instant::now();
            loop {
                if let Some(status) = child.try_wait()? {
                    result.exit_code = status.code();
                    break;
                }
                if start.elapsed() >= Duration::from_secs(policy.timeout_secs) {
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    let status = child.wait()?;
                    result.exit_code = status.code();
                    result.timed_out = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    if crate::git::head(Path::new(".")) != result.revision {
        result.error = Some(
            "verification changed HEAD; its receipt does not verify the resulting revision".into(),
        );
    }
    runtime::write_json(&evidence.join("verification.json"), &result)?;
    Ok(result)
}

/// Small feedback excerpt; complete command output stays in the attempt directory.
pub fn failure_text(receipt: &Receipt, evidence: &Path) -> String {
    use std::io::Read;
    let mut out = format!(
        "Verification failed: {:?}; exit={:?}; timeout={}; {}",
        receipt.command,
        receipt.exit_code,
        receipt.timed_out,
        receipt.error.as_deref().unwrap_or("")
    );
    for name in ["verify.stderr", "verify.stdout"] {
        let mut bytes = Vec::new();
        if let Ok(f) = std::fs::File::open(evidence.join(name)) {
            let _ = f.take(2048).read_to_end(&mut bytes);
        }
        let text = String::from_utf8_lossy(&bytes);
        if !text.is_empty() {
            out.push_str(&format!("\n{name}:\n{text}"));
        }
    }
    out
}

pub fn review(
    cfg: &Config,
    contract: &str,
    summary: &str,
    before: &Option<String>,
    evidence: &Path,
) -> R<judge::Decision> {
    let diff = match (before.as_deref(), crate::git::head(Path::new("."))) {
        (Some(old), Some(new)) => crate::git::range_diff_text(Path::new("."), old, &new, 24 * 1024),
        _ => String::new(),
    };
    let prompt = judge::build_prompt("Frozen task contract", contract, summary, &diff);
    std::fs::write(evidence.join("review-prompt.md"), &prompt)?;
    let result = crate::synth::run_oneshot(cfg, &cfg.judge_model, 180, &prompt);
    std::fs::write(
        evidence.join("review.txt"),
        result.as_deref().unwrap_or("Review unavailable"),
    )?;
    let decision = result
        .as_deref()
        .map(judge::parse_decision)
        .unwrap_or(judge::Decision::Unavailable);
    runtime::write_json(&evidence.join("review.json"), &decision)?;
    Ok(decision)
}
