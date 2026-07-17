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

    /// The session id for a claude instance. The top-level `claude` process does
    /// NOT carry `CLAUDE_CODE_SESSION_ID` in its own environment (Claude Code
    /// injects it into the child processes it spawns), so we scan descendants —
    /// nearest first — and return the first one that exposes it.
    fn session_id_of(pid: u32, ppid_of: &HashMap<u32, u32>) -> Option<String> {
        for d in discover::descendants(pid, ppid_of) {
            if let Some(env) = Self::env_of(d) {
                if let Some(sid) = discover::parse_env_var(&env, "CLAUDE_CODE_SESSION_ID") {
                    return Some(sid);
                }
            }
        }
        None
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
            // CLAUDE_CONFIG_DIR (if the user set it) is inherited from launch, so
            // it lives on the claude process's own env. The session id does NOT —
            // it is read from a descendant (see session_id_of).
            let env = Self::env_of(p.pid).unwrap_or_default();
            let session_id = Self::session_id_of(p.pid, &ppid_of);
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
                session_cost: None,   // filled from usage snapshot in main
                session_id: session_id.clone(),
            });
        }
        rows.sort_by_key(|r| r.pid);
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
