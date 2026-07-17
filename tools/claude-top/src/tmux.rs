//! tmux pane discovery. A Claude process's pane is found by walking up the
//! parent-pid chain until we hit a pid that owns a tmux pane.

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane { pub pane_pid: u32, pub label: String, pub path: String }

/// Parse `tmux list-panes -a -F '#{pane_pid} #{session_name}:#{window_index}.#{pane_index} #{pane_current_path}'`.
pub fn parse_panes(out: &str) -> Vec<Pane> {
    out.lines().filter_map(|line| {
        let mut it = line.split_whitespace();
        let pane_pid = it.next()?.parse::<u32>().ok()?;
        let label = it.next()?.to_string();
        let path = it.collect::<Vec<_>>().join(" ");
        Some(Pane { pane_pid, label, path })
    }).collect()
}

/// Walk the ppid chain from `pid` upward; return the first ancestor (or self)
/// that owns a pane. Guards against cycles and pid 0/1 roots.
pub fn pane_for_pid(pid: u32, ppid_of: &HashMap<u32, u32>, panes: &[Pane]) -> Option<Pane> {
    let by_pid: HashMap<u32, &Pane> = panes.iter().map(|p| (p.pane_pid, p)).collect();
    let mut cur = pid;
    let mut seen = HashSet::new();
    while cur > 1 && seen.insert(cur) {
        if let Some(p) = by_pid.get(&cur) { return Some((*p).clone()); }
        match ppid_of.get(&cur) { Some(&parent) => cur = parent, None => break }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_panes() {
        let out = "197 0:0.0 /Users/d/suitcase\n8204 1:0.1 /Users/d/discord\n";
        let panes = parse_panes(out);
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0], Pane { pane_pid: 197, label: "0:0.0".into(), path: "/Users/d/suitcase".into() });
        assert_eq!(panes[1], Pane { pane_pid: 8204, label: "1:0.1".into(), path: "/Users/d/discord".into() });
    }

    #[test]
    fn parses_panes_with_spaces_in_path() {
        let out = "100 2:0.0 /Users/d/my code/dir\n";
        let panes = parse_panes(out);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0], Pane { pane_pid: 100, label: "2:0.0".into(), path: "/Users/d/my code/dir".into() });
    }

    #[test]
    fn pane_for_pid_handles_cycles() {
        let panes = vec![];
        let mut ppid = HashMap::new();
        ppid.insert(10u32, 20u32);
        ppid.insert(20u32, 10u32);
        // Should not infinite loop and should return None since neither pid owns a pane
        assert!(pane_for_pid(10, &ppid, &panes).is_none());
        assert!(pane_for_pid(20, &ppid, &panes).is_none());
    }

    #[test]
    fn maps_pid_via_ancestor_walk() {
        // claude(1634) -> shell(197 == pane_pid). Also a deeper chain: 999 -> 500 -> 8204.
        let panes = parse_panes("197 0:0.0 /a\n8204 1:0.1 /b\n");
        let mut ppid = HashMap::new();
        ppid.insert(1634u32, 197u32);
        ppid.insert(999u32, 500u32);
        ppid.insert(500u32, 8204u32);
        assert_eq!(pane_for_pid(1634, &ppid, &panes).unwrap().label, "0:0.0");
        assert_eq!(pane_for_pid(999, &ppid, &panes).unwrap().label, "1:0.1");
        assert!(pane_for_pid(42, &ppid, &panes).is_none());
    }
}
