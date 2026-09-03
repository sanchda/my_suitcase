//! The bridge from ralphd to the `ralph` binary. Every command shells out —
//! ralphd never reaches around the CLI into `.ralph/`, so a terminal can do
//! everything Discord can. The short calls run on a blocking pool: with several
//! loops polling on a 30s cadence, `Command::output()` on a gateway worker can
//! stall the heartbeat and blow Discord's 3s interaction-ack window.

use crate::config::{self, LoopConfig};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

#[derive(Clone)]
pub struct Ralph {
    working_dir: PathBuf,
    ralph_args: Vec<String>,
    webhook: Option<String>,
    /// Environment aiming the short calls at this loop's own state dir and
    /// `ralph.toml`; see [`relocation`].
    relocation: Vec<(&'static str, PathBuf)>,
}

/// Result of a short `ralph` invocation.
pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

fn argv<const N: usize>(parts: [&str; N]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// A loop launched with `--dir` / `--config` / `--backlog` needs the short calls
/// aimed at the same places, or `/status` reports on `<repo>/.ralph` while the
/// loop lives in `/var/lib/ralph/x`. Re-passing the flags does not achieve that:
/// only the loop itself, `stop` and `msg` read them from argv — `status`, `add`,
/// `done`, `uncheck`, `drop`, `model` and `backlog edit` resolve their paths
/// through ralph's `load_base`, which consults the config file and the
/// environment and silently ignores a `--dir` sitting in argv. The `RALPH_*`
/// variables are the one channel every subcommand honors.
///
/// `dir` and `backlog` are independent in ralph, so relocating one never moves
/// the other and each has to be forwarded on its own.
///
/// Only when the loop's own args move them: a loop that relocates nothing must
/// keep inheriting ralphd's environment — and any `dir` in its `ralph.toml` —
/// exactly as the spawned loop child does.
fn relocation(cfg: &LoopConfig) -> Vec<(&'static str, PathBuf)> {
    let mut env = Vec::new();
    if config::forwarded_path(&cfg.ralph_args, "--dir").is_some() {
        env.push(("RALPH_DIR", cfg.state_dir.clone()));
    }
    if config::forwarded_path(&cfg.ralph_args, "--config").is_some() {
        env.push(("RALPH_CONFIG", cfg.ralph_config.clone()));
    }
    // Not pre-resolved by config.rs: ralphd never reads the backlog itself.
    if let Some(p) = config::forwarded_path(&cfg.ralph_args, "--backlog") {
        let p = if p.is_absolute() {
            p
        } else {
            cfg.working_dir.join(p)
        };
        env.push(("RALPH_BACKLOG", p));
    }
    env
}

impl Ralph {
    pub fn new(cfg: &LoopConfig) -> Self {
        Ralph {
            working_dir: cfg.working_dir.clone(),
            ralph_args: cfg.ralph_args.clone(),
            webhook: cfg.webhook.clone(),
            relocation: relocation(cfg),
        }
    }

    async fn run(&self, args: Vec<String>) -> std::io::Result<Output> {
        let dir = self.working_dir.clone();
        let env = self.relocation.clone();
        tokio::task::spawn_blocking(move || {
            let out = Command::new("ralph")
                .args(&args)
                .envs(env.iter().map(|(k, v)| (*k, v)))
                .current_dir(&dir)
                .output()?;
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
    /// `claude` NDJSON stream passes through on stdout. stderr is discarded;
    /// diagnostics also surface in the result envelope. The session can run for
    /// many minutes, so callers must defer the interaction and drive the stream
    /// (see `crate::msg`).
    pub fn spawn_msg(
        &self,
        text: &str,
        new: bool,
        model: Option<&str>,
    ) -> std::io::Result<tokio::process::Child> {
        let mut cmd = tokio::process::Command::new("ralph");
        cmd.arg("msg");
        if new {
            cmd.arg("--new");
        }
        // Sticky on ralph's side, so omitting it keeps whatever the thread is on.
        if let Some(m) = model.map(str::trim).filter(|m| !m.is_empty()) {
            cmd.arg("--model").arg(m);
        }
        // Raw NDJSON passthrough: msg.rs folds claude's own event stream, so the
        // human-readable default would leave it with nothing to parse.
        cmd.arg("--stream-json")
            .arg(text)
            .envs(self.relocation.iter().map(|(k, v)| (*k, v)))
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
    use std::path::Path;

    /// A loop as `config.rs` would resolve it from `args`.
    fn loop_with(args: &[&str]) -> LoopConfig {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        LoopConfig {
            name: "grove".into(),
            channel_id: 2,
            state_dir: config::resolve_state_dir(Path::new("/repo"), &args),
            ralph_config: config::resolve_ralph_config(Path::new("/repo"), &args),
            working_dir: PathBuf::from("/repo"),
            ralph_args: args,
            webhook: Some("https://hook".into()),
            autostart: false,
        }
    }

    #[test]
    fn new_captures_the_loops_launch_profile() {
        let r = Ralph::new(&loop_with(&["--model", "opus"]));
        assert_eq!(r.working_dir, PathBuf::from("/repo"));
        assert_eq!(r.ralph_args, vec!["--model".to_string(), "opus".to_string()]);
        assert_eq!(r.webhook.as_deref(), Some("https://hook"));
    }

    #[test]
    fn short_calls_are_aimed_at_a_relocated_state_dir() {
        let r = Ralph::new(&loop_with(&["--dir", "/var/lib/ralph/x"]));
        assert_eq!(
            r.relocation,
            vec![("RALPH_DIR", PathBuf::from("/var/lib/ralph/x"))]
        );
        // A relative --dir travels resolved, since the callee's own default is
        // relative to its cwd and would otherwise be re-relativized.
        let r = Ralph::new(&loop_with(&["--dir", "state"]));
        assert_eq!(r.relocation, vec![("RALPH_DIR", PathBuf::from("/repo/state"))]);

        let r = Ralph::new(&loop_with(&["--config", "/etc/ralph.toml"]));
        assert_eq!(
            r.relocation,
            vec![("RALPH_CONFIG", PathBuf::from("/etc/ralph.toml"))]
        );

        // ralph resolves `backlog` independently of `dir`, so a loop that moves
        // only the backlog still needs it forwarded on its own.
        let r = Ralph::new(&loop_with(&["--backlog", "docs/PLAN.md"]));
        assert_eq!(
            r.relocation,
            vec![("RALPH_BACKLOG", PathBuf::from("/repo/docs/PLAN.md"))]
        );
    }

    #[test]
    fn a_loop_that_relocates_nothing_is_left_alone() {
        // No --dir/--config means ralph's own resolution (its config file, then
        // ralphd's environment) must keep deciding — pinning the defaults here
        // would override a `dir` set in the repo's ralph.toml.
        assert!(Ralph::new(&loop_with(&["--model", "opus"])).relocation.is_empty());
        assert!(Ralph::new(&loop_with(&[])).relocation.is_empty());
    }
}
