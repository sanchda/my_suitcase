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

    /// When `pid` started. A session whose last activity predates this cannot
    /// belong to this process, which is what makes the cwd fallback safe: a
    /// freshly-launched instance can't adopt a finished session's totals, while
    /// an instance idle for days still matches its own old transcript.
    fn started_at(pid: u32) -> Option<chrono::DateTime<chrono::Utc>> {
        let out = Self::sh("ps", &["-o", "etime=", "-p", &pid.to_string()])?;
        let secs = discover::parse_etime(&out)?;
        Some(chrono::Utc::now() - chrono::Duration::seconds(secs as i64))
    }

    fn cwd_of(pid: u32) -> Option<PathBuf> {
        let out = Self::sh("lsof", &["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])?;
        out.lines().find_map(|l| l.strip_prefix('n')).map(PathBuf::from)
    }

    /// Read one environment variable of another process.
    ///
    /// Platform-specific by necessity: `ps -E` is a BSD/macOS spelling for
    /// "append the environment", and procps-ng rejects it outright (`error:
    /// unsupported SysV option`, exit 1), which silently blanked every
    /// env-derived field on Linux. Linux exposes the same data far more
    /// precisely via `/proc/<pid>/environ`, which needs no subprocess at all.
    #[cfg(target_os = "linux")]
    fn env_var_of(pid: u32, key: &str) -> Option<String> {
        // Readable for same-uid processes, which is all claude-top reports on.
        let raw = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
        discover::parse_env_nul(&String::from_utf8_lossy(&raw), key)
    }

    #[cfg(not(target_os = "linux"))]
    fn env_var_of(pid: u32, key: &str) -> Option<String> {
        let out = Self::sh("ps", &["-Eww", "-o", "command=", "-p", &pid.to_string()])?;
        discover::parse_env_var(&out, key)
    }

    /// The session id for a claude instance, read from the process tree. The
    /// top-level `claude` process does NOT carry `CLAUDE_CODE_SESSION_ID` in its
    /// own environment (Claude Code injects it into the child processes it
    /// spawns), so we scan descendants — nearest first — and return the first
    /// one that exposes it.
    ///
    /// Returns None whenever the instance has no live children, which is the
    /// common case for an idle instance; callers fall back to matching on cwd.
    fn session_id_of(pid: u32, ppid_of: &HashMap<u32, u32>) -> Option<String> {
        discover::descendants(pid, ppid_of)
            .into_iter()
            .find_map(|d| Self::env_var_of(d, "CLAUDE_CODE_SESSION_ID"))
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
        for p in procs.iter().filter(|p| discover::is_claude(&p.command)) {
            // CLAUDE_CONFIG_DIR (if the user set it) is inherited from launch, so
            // it lives on the claude process's own env. The session id does NOT —
            // it is read from a descendant (see session_id_of).
            let session_id = Self::session_id_of(p.pid, &ppid_of);
            let config_dir = Self::env_var_of(p.pid, "CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| default_dir.clone());
            let account = Self::account_for(&config_dir);
            let pane = tmux::pane_for_pid(p.pid, &ppid_of, &panes);
            let dir = Self::cwd_of(p.pid).or_else(|| pane.as_ref().map(|p| PathBuf::from(&p.path)));
            let gi = dir.as_ref().map(|d| git::git_info(d)).unwrap_or_default();
            rows.push((dir.clone(), InstanceRow {
                pid: p.pid,
                account,
                tmux: pane.as_ref().map(|p| p.label.clone()),
                dir: dir.map(|d| shorten_home(&d)),
                branch: gi.branch,
                worktree: gi.worktree,
                model: None,          // filled from usage snapshot in main
                session_tokens: 0,    // filled from usage snapshot in main
                session_cost: None,   // filled from usage snapshot in main
                session_id,
            }));
        }
        rows.sort_by_key(|(_, r)| r.pid);

        // Ids read from the process tree are exact, so claim them all before
        // resolving anything by cwd — otherwise a guess could take the session
        // that a later row knows for certain is its own.
        let mut claimed: std::collections::HashSet<String> =
            rows.iter().filter_map(|(_, r)| r.session_id.clone()).collect();
        for (dir, row) in rows.iter_mut() {
            if row.session_id.is_some() { continue; }
            let Some(dir) = dir else { continue };
            // If the start time can't be read the process is on its way out, so
            // don't let an unknown bound silently widen the match.
            let Some(active_since) = Self::started_at(row.pid) else { continue };
            // One session per instance: two instances in the same directory take
            // the two most recently active sessions there rather than both
            // reporting the same totals.
            let pick = self.collector.sessions_for_cwd(dir, active_since)
                .into_iter()
                .find(|sid| !claimed.contains(sid));
            if let Some(sid) = pick {
                claimed.insert(sid.clone());
                row.session_id = Some(sid);
            }
        }

        let rows: Vec<InstanceRow> = rows.into_iter().map(|(_, r)| r).collect();
        let session_ids = rows.iter().filter_map(|r| r.session_id.clone()).collect();
        (rows, session_ids, header_account, notes.join(" · "))
    }

    pub fn refresh_usage(&mut self, config_dir: &Path) { self.collector.refresh_dir(config_dir); }
    pub fn usage_snapshot(&self, window: usage::Window, running: &std::collections::HashSet<String>) -> usage::UsageSnapshot {
        self.collector.snapshot(window, chrono::Local::now().date_naive(), running)
    }
}

fn shorten_home(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    if let Ok(home) = std::env::var("HOME") {
        if let Some(rest) = s.strip_prefix(&home) {
            if rest.is_empty() || rest.starts_with('/') { return format!("~{rest}"); }
        }
    }
    s
}
