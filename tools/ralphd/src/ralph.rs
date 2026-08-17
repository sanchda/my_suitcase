//! The bridge from ralphd to the `ralph` binary. Every command shells out —
//! ralphd never reaches around the CLI into `.ralph/`, so a terminal can do
//! everything Discord can. The short calls run on a blocking pool: with several
//! loops polling on a 30s cadence, `Command::output()` on a gateway worker can
//! stall the heartbeat and blow Discord's 3s interaction-ack window.

use crate::config::LoopConfig;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

#[derive(Clone)]
pub struct Ralph {
    working_dir: PathBuf,
    ralph_args: Vec<String>,
    webhook: Option<String>,
}

/// Result of a short `ralph` invocation.
pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Build an owned argv from string-ish parts.
fn argv<const N: usize>(parts: [&str; N]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

impl Ralph {
    pub fn new(cfg: &LoopConfig) -> Self {
        Ralph {
            working_dir: cfg.working_dir.clone(),
            ralph_args: cfg.ralph_args.clone(),
            webhook: cfg.webhook.clone(),
        }
    }

    async fn run(&self, args: Vec<String>) -> std::io::Result<Output> {
        let dir = self.working_dir.clone();
        tokio::task::spawn_blocking(move || {
            let out = Command::new("ralph").args(&args).current_dir(&dir).output()?;
            Ok(Output {
                ok: out.status.success(),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        })
        .await
        .map_err(|e| std::io::Error::other(format!("ralph call panicked: {e}")))?
    }

    /// `ralph status --json` → the raw JSON line on stdout.
    pub async fn status_json(&self) -> std::io::Result<Output> {
        self.run(argv(["status", "--json"])).await
    }

    /// `ralph stop [--now]` — graceful halt after the current iteration, or
    /// `--now` to also signal the running loop.
    pub async fn stop(&self, now: bool) -> std::io::Result<Output> {
        let mut a = argv(["stop"]);
        if now {
            a.push("--now".into());
        }
        self.run(a).await
    }

    /// `ralph model <tier>` — validation lives in `ralph`, which owns the file.
    pub async fn model(&self, tier: &str) -> std::io::Result<Output> {
        self.run(vec!["model".into(), tier.into()]).await
    }

    /// `ralph add [--under <parent>] [<id>] <title> [--verify <cmd>]`.
    pub async fn add(
        &self,
        id: Option<&str>,
        title: &str,
        verify: Option<&str>,
        under: Option<&str>,
    ) -> std::io::Result<Output> {
        let mut a = argv(["add"]);
        if let Some(p) = under {
            a.push("--under".into());
            a.push(p.into());
        }
        if let Some(i) = id {
            a.push(i.into());
        }
        a.push(title.into());
        if let Some(v) = verify {
            a.push("--verify".into());
            a.push(v.into());
        }
        self.run(a).await
    }

    /// `ralph drop <id> [--recursive]`.
    pub async fn drop_task(&self, id: &str, recursive: bool) -> std::io::Result<Output> {
        let mut a = vec!["drop".to_string(), id.to_string()];
        if recursive {
            a.push("--recursive".into());
        }
        self.run(a).await
    }

    pub async fn uncheck(&self, id: &str) -> std::io::Result<Output> {
        self.run(vec!["uncheck".into(), id.into()]).await
    }

    pub async fn done(&self, id: &str) -> std::io::Result<Output> {
        self.run(vec!["done".into(), id.into()]).await
    }

    pub async fn backlog_edit(
        &self,
        id: &str,
        title: &str,
        verify: &str,
    ) -> std::io::Result<Output> {
        self.run(vec![
            "backlog".into(),
            "edit".into(),
            "--id".into(),
            id.into(),
            "--title".into(),
            title.into(),
            "--verify".into(),
            verify.into(),
        ])
        .await
    }

    /// Spawn the loop: `ralph <forwarded args> <extra_args>` in the working dir.
    /// `extra_args` (e.g. a `/start` model override) are appended, so they win
    /// over the launch profile. The child is detached from stdio and NOT waited
    /// on; the caller records it under its channel.
    pub fn spawn_loop(&self, extra_args: &[String]) -> std::io::Result<Child> {
        let mut cmd = Command::new("ralph");
        cmd.args(&self.ralph_args)
            .args(extra_args)
            .current_dir(&self.working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // ralph reads DISCORD_WEBHOOK from the environment only, so an inherited
        // one would send every loop's lifecycle posts to one channel. Set it per
        // child, and clear it when this loop has none rather than leak ralphd's.
        match &self.webhook {
            Some(url) => cmd.env("DISCORD_WEBHOOK", url),
            None => cmd.env_remove("DISCORD_WEBHOOK"),
        };
        // Run the loop in its OWN session so ralph's process tree — and the
        // `kill -9 -<pgid>` sweeps it fires between iterations — can never climb
        // back up and take ralphd down with it. Without this, ralphd and ralph
        // share one process group and a group-kill of the iteration subtree nukes
        // ralphd too (observed: one `kill` SIGKILLing ralphd + both ralph procs).
        #[cfg(unix)]
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        cmd.spawn()
    }

    /// Spawn `ralph msg [--new] <text>` — a persistent steering session whose
    /// `claude` NDJSON stream passes through on stdout, so the caller can fold
    /// live token usage and the final cost from it. stderr is discarded
    /// (diagnostics also surface in the result envelope). The session can run for
    /// many minutes, so callers must defer the interaction and drive the stream
    /// (see `crate::msg`).
    pub fn spawn_msg(&self, text: &str, new: bool) -> std::io::Result<tokio::process::Child> {
        let mut cmd = tokio::process::Command::new("ralph");
        cmd.arg("msg");
        if new {
            cmd.arg("--new");
        }
        // Raw NDJSON passthrough: msg.rs folds claude's own event stream, so the
        // human-readable default would leave it with nothing to parse.
        cmd.arg("--stream-json")
            .arg(text)
            .current_dir(&self.working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        cmd.spawn()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_captures_the_loops_launch_profile() {
        let cfg = LoopConfig {
            name: "grove".into(),
            channel_id: 2,
            working_dir: PathBuf::from("/repo"),
            state_dir: PathBuf::from("/repo/.ralph"),
            ralph_config: PathBuf::from("/repo/.ralph/ralph.toml"),
            ralph_args: vec!["--model".into(), "opus".into()],
            webhook: Some("https://hook".into()),
            autostart: false,
        };
        let r = Ralph::new(&cfg);
        assert_eq!(r.working_dir, PathBuf::from("/repo"));
        assert_eq!(r.ralph_args, vec!["--model".to_string(), "opus".to_string()]);
        assert_eq!(r.webhook.as_deref(), Some("https://hook"));
    }
}
