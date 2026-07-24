//! Discovering local Claude Code instances and their identity. Pure parsers are
//! unit-tested here; the functions that actually shell out to `ps` live in
//! `runtime` (Task 9 wires them in) and are intentionally thin.

use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub command: String,
}

/// Parse `ps -axo pid,ppid,command` output (header line tolerated/skipped).
pub fn parse_ps(out: &str) -> Vec<Proc> {
    let mut v = Vec::new();
    for line in out.lines() {
        let mut parts = line.split_whitespace();
        let pid = parts.next().and_then(|s| s.parse::<u32>().ok());
        let ppid = parts.next().and_then(|s| s.parse::<u32>().ok());
        let (pid, ppid) = match (pid, ppid) {
            (Some(pid), Some(ppid)) => (pid, ppid),
            _ => continue, // tolerates the `PID PPID COMMAND` header line
        };
        let command = parts.collect::<Vec<_>>().join(" ");
        v.push(Proc { pid, ppid, command });
    }
    v
}

/// True if the command's argv0 basename is exactly `claude`.
pub fn is_claude(command: &str) -> bool {
    let argv0 = command.split_whitespace().next().unwrap_or("");
    let base = argv0.rsplit('/').next().unwrap_or(argv0);
    base == "claude"
}

/// Extract KEY=value from a `ps -Eww` command string (env appended after argv).
///
/// This is the BSD/macOS route; Linux reads `/proc/<pid>/environ` via
/// `parse_env_nul` instead, so on Linux nothing but the test calls this. Kept
/// compiled and tested on every platform since it's a pure parser.
#[allow(dead_code)]
pub fn parse_env_var(command: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    command
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix(&needle))
        .map(|s| s.to_string())
}

/// Extract KEY=value from NUL-separated environment data, i.e. the contents of
/// Linux `/proc/<pid>/environ`. Preferred over `parse_env_var` where available:
/// entries are delimited exactly, so values containing spaces survive intact.
pub fn parse_env_nul(raw: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    raw.split('\0')
        .find_map(|entry| entry.strip_prefix(&needle))
        .map(|s| s.to_string())
}

/// Parse `ps -o etime=` output (elapsed time since process start) into seconds.
/// POSIX renders it as `[[dd-]hh:]mm:ss`, right-aligned in a padded field.
pub fn parse_etime(s: &str) -> Option<u64> {
    let s = s.trim();
    let (days, hms) = match s.split_once('-') {
        Some((d, rest)) => (d.parse::<u64>().ok()?, rest),
        None => (0, s),
    };
    let mut parts = hms.split(':').map(|p| p.parse::<u64>().ok());
    let (a, b, c) = (parts.next()?, parts.next()?, parts.next());
    if parts.next().is_some() { return None; } // more than hh:mm:ss
    let (h, m, sec) = match c {
        Some(c) => (a?, b?, c?),      // hh:mm:ss
        None => (0, a?, b?),          // mm:ss
    };
    Some(days * 86400 + h * 3600 + m * 60 + sec)
}

/// Read `oauthAccount.emailAddress` from a `.claude.json` string.
pub fn account_email(config_json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(config_json).ok()?;
    v.get("oauthAccount")?
        .get("emailAddress")?
        .as_str()
        .map(|s| s.to_string())
}

/// Config dir: `$CLAUDE_CONFIG_DIR` else `$HOME/.claude`.
pub fn default_config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".claude")
}

/// All descendants of `pid`, breadth-first (nearest first), excluding `pid`
/// itself. `ppid_of` maps each pid to its parent (as built from `parse_ps`).
///
/// Used to find a session id: the top-level `claude` process does NOT carry
/// `CLAUDE_CODE_SESSION_ID` in its own environment — Claude Code injects it into
/// the child processes it spawns — so we read it from a descendant instead.
/// BFS (nearest-first) so a direct helper child is preferred over anything a
/// nested subagent might spawn deeper down.
pub fn descendants(pid: u32, ppid_of: &HashMap<u32, u32>) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&child, &parent) in ppid_of {
        children.entry(parent).or_default().push(child);
    }
    for kids in children.values_mut() {
        kids.sort_unstable(); // deterministic ordering among siblings
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    seen.insert(pid);
    queue.push_back(pid);
    while let Some(cur) = queue.pop_front() {
        if let Some(kids) = children.get(&cur) {
            for &k in kids {
                if seen.insert(k) {
                    out.push(k);
                    queue.push_back(k);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descendants_breadth_first_excludes_self_and_unrelated() {
        // tree: 1 -> {2,3}; 2 -> {4}; 4 -> {5}; plus unrelated 9 -> 99
        let ppid_of: HashMap<u32, u32> =
            [(2, 1), (3, 1), (4, 2), (5, 4), (9, 99)].into_iter().collect();
        assert_eq!(descendants(1, &ppid_of), vec![2, 3, 4, 5]);
        assert!(descendants(1, &ppid_of).iter().all(|&d| d != 1)); // excludes self
        assert!(!descendants(1, &ppid_of).contains(&9)); // excludes unrelated
        assert_eq!(descendants(7, &ppid_of), Vec::<u32>::new()); // no children
    }

    #[test]
    fn parses_ps_and_filters_claude() {
        let out = "  PID  PPID COMMAND\n 1634   197 claude --dangerously-skip-permissions\n 5346  1634 /bin/bash -c source ...\n";
        let procs = parse_ps(out);
        let claude: Vec<_> = procs.iter().filter(|p| is_claude(&p.command)).collect();
        assert_eq!(claude.len(), 1);
        assert_eq!(claude[0].pid, 1634);
        assert_eq!(claude[0].ppid, 197);
    }

    #[test]
    fn parses_ps_etime_in_all_documented_widths() {
        // POSIX `ps -o etime=` renders [[dd-]hh:]mm:ss, space-padded.
        assert_eq!(parse_etime("      03:35"), Some(3 * 60 + 35));
        assert_eq!(parse_etime("   04:36:02"), Some(4 * 3600 + 36 * 60 + 2));
        assert_eq!(parse_etime(" 3-04:36:02"), Some(3 * 86400 + 4 * 3600 + 36 * 60 + 2));
        assert_eq!(parse_etime("00:00\n"), Some(0));
    }

    #[test]
    fn rejects_unparseable_etime() {
        assert_eq!(parse_etime(""), None);
        assert_eq!(parse_etime("-"), None);
        assert_eq!(parse_etime("garbage"), None);
        assert_eq!(parse_etime("1:2:3:4"), None);
    }

    #[test]
    fn parses_nul_separated_environ() {
        // Shape of Linux `/proc/<pid>/environ`: NUL-separated, no trailing key.
        let raw = "PATH=/usr/bin\0CLAUDE_CODE_SESSION_ID=abc-123\0HOME=/home/u\0";
        assert_eq!(parse_env_nul(raw, "CLAUDE_CODE_SESSION_ID"), Some("abc-123".into()));
        assert_eq!(parse_env_nul(raw, "HOME"), Some("/home/u".into()));
        assert_eq!(parse_env_nul(raw, "CLAUDE_CONFIG_DIR"), None);
    }

    #[test]
    fn nul_environ_keeps_values_containing_spaces() {
        // The whitespace-splitting `ps` parser can't do this; /proc data can.
        let raw = "SSH_CLIENT=10.0.0.1 55912 22\0CLAUDE_CONFIG_DIR=/opt/my claude\0";
        assert_eq!(parse_env_nul(raw, "SSH_CLIENT"), Some("10.0.0.1 55912 22".into()));
        assert_eq!(parse_env_nul(raw, "CLAUDE_CONFIG_DIR"), Some("/opt/my claude".into()));
    }

    #[test]
    fn nul_environ_does_not_match_key_as_substring() {
        let raw = "XCLAUDE_CONFIG_DIR=/wrong\0CLAUDE_CONFIG_DIRX=/wrong\0";
        assert_eq!(parse_env_nul(raw, "CLAUDE_CONFIG_DIR"), None);
    }

    #[test]
    fn env_and_account_parsing() {
        assert_eq!(
            parse_env_var("claude arg CLAUDE_CODE_SESSION_ID=abc-123 X=y", "CLAUDE_CODE_SESSION_ID"),
            Some("abc-123".into())
        );
        assert_eq!(parse_env_var("claude", "CLAUDE_CONFIG_DIR"), None);
        let json = r#"{"oauthAccount":{"emailAddress":"a@b.com","organizationName":"Org"}}"#;
        assert_eq!(account_email(json), Some("a@b.com".into()));
    }
}
