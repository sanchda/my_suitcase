//! The bridge from ralphd to the `ralph` binary and `.ralph/` files. All
//! blocking `ralph` calls here are fast (lint/status/backlog run in ms); only
//! `spawn_loop` starts a long-lived process, which we do NOT wait on.

use crate::config::BotConfig;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

pub struct Ralph {
    working_dir: PathBuf,
    state_dir: PathBuf,
    ralph_args: Vec<String>,
}

/// Result of a short `ralph` invocation.
pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Ralph {
    pub fn new(cfg: &BotConfig) -> Self {
        Ralph {
            working_dir: cfg.working_dir.clone(),
            state_dir: cfg.state_dir.clone(),
            ralph_args: cfg.ralph_args.clone(),
        }
    }

    fn run(&self, args: &[&str]) -> std::io::Result<Output> {
        let out = Command::new("ralph")
            .args(args)
            .current_dir(&self.working_dir)
            .output()?;
        Ok(Output {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    /// `ralph status --json` → the raw JSON line on stdout.
    pub fn status_json(&self) -> std::io::Result<Output> {
        self.run(&["status", "--json"])
    }

    /// `ralph stop` — graceful halt after the current iteration.
    pub fn stop(&self) -> std::io::Result<Output> {
        self.run(&["stop"])
    }

    pub fn backlog_add(&self, title: &str, verify: &str) -> std::io::Result<Output> {
        self.run(&["backlog", "add", "--title", title, "--verify", verify])
    }

    pub fn backlog_edit(&self, id: &str, title: &str, verify: &str) -> std::io::Result<Output> {
        self.run(&[
            "backlog", "edit", "--id", id, "--title", title, "--verify", verify,
        ])
    }

    /// Write the one-shot `.ralph/MODEL` override. `tier` MUST already be
    /// validated (see `model::validate_tier`).
    pub fn write_model(&self, tier: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.state_dir)?;
        std::fs::write(self.state_dir.join("MODEL"), format!("{tier}\n"))
    }

    /// Spawn the loop: `ralph <forwarded args> <extra_args>` in the working dir.
    /// `extra_args` (e.g. a `/start` model override) are appended, so they win
    /// over the launch profile. The child is detached from stdio and NOT waited
    /// on; the caller records its pid.
    pub fn spawn_loop(&self, extra_args: &[String]) -> std::io::Result<Child> {
        Command::new("ralph")
            .args(&self.ralph_args)
            .args(extra_args)
            .current_dir(&self.working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    }

    /// Run a one-off yolo `claude` session with `message` as the prompt,
    /// optionally pinned to `model`. This can take a while, so callers must defer
    /// the interaction first. Returns claude's final text, or an error string.
    pub async fn btw(&self, message: &str, model: Option<&str>) -> String {
        let mut cmd = tokio::process::Command::new("claude");
        cmd.arg("-p").arg("--dangerously-skip-permissions");
        if let Some(m) = model {
            cmd.arg("--model").arg(m);
        }
        cmd.arg(message).current_dir(&self.working_dir);
        match cmd.output().await {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if text.is_empty() {
                    "(claude produced no output)".to_string()
                } else {
                    text
                }
            }
            Ok(o) => format!("claude failed: {}", String::from_utf8_lossy(&o.stderr).trim()),
            Err(e) => format!("could not start claude: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_captures_config_paths() {
        let cfg = BotConfig {
            token: "t".into(),
            guild_id: 1,
            channel_id: 2,
            user_id: 3,
            working_dir: PathBuf::from("/repo"),
            state_dir: PathBuf::from("/repo/.ralph"),
            ralph_args: vec!["--model".into(), "opus".into()],
        };
        let r = Ralph::new(&cfg);
        assert_eq!(r.working_dir, PathBuf::from("/repo"));
        assert_eq!(r.state_dir, PathBuf::from("/repo/.ralph"));
        assert_eq!(
            r.ralph_args,
            vec!["--model".to_string(), "opus".to_string()]
        );
    }

    #[test]
    fn write_model_creates_the_file() {
        let dir = std::env::temp_dir().join(format!("ralphd-model-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = BotConfig {
            token: "t".into(),
            guild_id: 1,
            channel_id: 2,
            user_id: 3,
            working_dir: dir.clone(),
            state_dir: dir.clone(),
            ralph_args: vec![],
        };
        Ralph::new(&cfg).write_model("opus").unwrap();
        let written = std::fs::read_to_string(dir.join("MODEL")).unwrap();
        assert_eq!(written.trim(), "opus");
    }
}
