//! `ralph add|drop|done|uncheck` — the CLI's backlog mutations.
//!
//! Each one lints against the current backlog and refuses immediately if it
//! cannot apply, then queues the request. Who *drains* is the only thing left to
//! decide: a live loop drains at its next iteration boundary, and with no loop
//! running the CLI drains itself so terminal use stays instant.

use crate::backlog::Document;
use crate::backlog_edit::{empty_backlog, verify_body};
use crate::inbox::{self, Request};
use crate::{config, pidguard, supervisor, R};
use std::io::{IsTerminal, Read};
use std::path::Path;

/// A name clash, distinct from a schema rejection (1) so a caller can tell them
/// apart without reading the message.
pub const EXIT_CONFLICT: i32 = 3;

const USAGE: &str = "\
Usage: ralph add [<id>] <title> [--verify <cmd>] [--under <parent>]
       ralph drop <id> [--recursive]
       ralph done <id>
       ralph uncheck <id>

An explicit <id> inserts under the parent it implies (3.1.1 → under 3.1);
--under <parent> numbers the next free stage for you. With no id the task is
appended at top level. The body is `--verify <cmd>`, or — when --verify is
omitted and stdin is piped — the full text (prose plus a Verify: line) on stdin.
";

/// Our own flags, split from the config flags `load_base` still needs to see.
struct Args {
    positional: Vec<String>,
    verify: Option<String>,
    under: Option<String>,
    recursive: bool,
    help: bool,
    rest: Vec<String>,
}

/// Split argv. A `--flag value` we don't own is forwarded whole, so `--config`
/// and `--dir` keep working alongside positional arguments.
fn parse(argv: &[String]) -> R<Args> {
    let mut args = Args {
        positional: Vec::new(),
        verify: None,
        under: None,
        recursive: false,
        help: false,
        rest: Vec::new(),
    };
    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].as_str();
        let value = || {
            argv.get(i + 1)
                .cloned()
                .ok_or_else(|| format!("{a} needs a value"))
        };
        match a {
            "--verify" => {
                args.verify = Some(value()?);
                i += 2;
            }
            "--under" => {
                args.under = Some(value()?);
                i += 2;
            }
            "--recursive" | "-r" => {
                args.recursive = true;
                i += 1;
            }
            "-h" | "--help" => {
                args.help = true;
                i += 1;
            }
            _ if a.starts_with("--") => {
                args.rest.push(argv[i].clone());
                if let Some(v) = argv.get(i + 1).filter(|v| !v.starts_with("--")) {
                    args.rest.push(v.clone());
                    i += 1;
                }
                i += 1;
            }
            _ => {
                args.positional.push(argv[i].clone());
                i += 1;
            }
        }
    }
    Ok(args)
}

/// A leading positional is an id only if it looks like one — ids are digits and
/// dots, and a title never is.
fn looks_like_id(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '_' || c == '-')
        && token.chars().any(|c| c.is_ascii_digit())
}

/// The task body: `--verify`, else the full text piped on stdin. `--verify`
/// short-circuits the read because an inherited stdin that never closes (ralphd
/// shells out to us) would otherwise block the call forever.
fn read_body(verify: Option<&str>) -> R<String> {
    if let Some(verify) = verify {
        return Ok(verify_body(verify));
    }
    if !std::io::stdin().is_terminal() {
        let mut piped = String::new();
        std::io::stdin().read_to_string(&mut piped)?;
        if !piped.trim().is_empty() {
            return Ok(piped);
        }
    }
    Err("add: --verify <cmd> required (or pipe the body on stdin)".into())
}

/// The backlog as it stands, or the skeleton `add` bootstraps onto.
pub fn current(backlog: &Path) -> R<String> {
    match std::fs::read_to_string(backlog) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(empty_backlog()),
        Err(e) => Err(format!("{}: cannot read backlog: {e}", backlog.display()).into()),
    }
}

/// Build the request for `sub`, or the exit code that refuses it outright.
fn request(sub: &str, args: &Args, text: &str, body: String) -> R<Result<Request, i32>> {
    let p = &args.positional;
    let mut positional = p.iter().cloned();
    match sub {
        "add" => {
            // Trailing words join the title, so an unquoted title still works.
            let (id, title) = match p.len() {
                0 => return Err("add: <title> required".into()),
                1 if looks_like_id(&p[0]) && args.under.is_none() => {
                    return Err(format!("add: id `{}` given without a title", p[0]).into())
                }
                1 => (None, p[0].clone()),
                _ if looks_like_id(&p[0]) => (Some(p[0].clone()), p[1..].join(" ")),
                _ => (None, p.join(" ")),
            };
            if let Some(id) = &id {
                if let Some(existing) = Document::parse(text).tasks.iter().find(|t| t.id == *id) {
                    eprintln!("ralph add: id {id} already exists (line {})", existing.line);
                    return Ok(Err(EXIT_CONFLICT));
                }
            }
            Ok(Ok(Request::Add {
                id,
                under: args.under.clone(),
                title,
                body,
            }))
        }
        "drop" => Ok(Ok(Request::Drop {
            id: positional.next().ok_or("drop: <id> required")?,
            recursive: args.recursive,
        })),
        "done" => Ok(Ok(Request::Done {
            id: positional.next().ok_or("done: <id> required")?,
        })),
        "uncheck" => Ok(Ok(Request::Uncheck {
            id: positional.next().ok_or("uncheck: <id> required")?,
        })),
        other => Err(format!("unknown backlog command `{other}`").into()),
    }
}

/// Refuse to drop the leaf the running loop is working — it would pull the task
/// out from under the iteration in flight.
fn guards_the_selected_leaf(req: &Request, text: &str, loop_pid: Option<u32>) -> Option<String> {
    let (Request::Drop { id, .. }, Some(pid)) = (req, loop_pid) else {
        return None;
    };
    let doc = Document::parse(text);
    let selected = doc.selected_index().map(|i| doc.tasks[i].id.as_str());
    (selected == Some(id.as_str()))
        .then(|| format!("task `{id}` is the leaf the running loop (pid {pid}) selected"))
}

/// Lint, queue, and then either leave the drain to the running loop or do it
/// here. Every CLI mutation — including the `ralph backlog` aliases — lands via
/// this one path.
pub fn submit(cfg: &config::Config, text: &str, req: Request, label: &str) -> R<i32> {
    let loop_pid = pidguard::running(&supervisor::pidfile(&cfg.dir));
    if let Some(reason) = guards_the_selected_leaf(&req, text, loop_pid) {
        eprintln!("ralph {label} refused: {reason}");
        return Ok(1);
    }
    // Lint against the backlog as it stands now: a request that cannot apply
    // fails here with a real error rather than silently at drain time.
    if let Err(e) = req.apply(text) {
        eprintln!("ralph {label} rejected:\n{e}");
        return Ok(1);
    }
    let queued = inbox::enqueue(&cfg.dir, &req)?;
    if loop_pid.is_some() {
        println!("queued (applies at the next iteration boundary)");
        return Ok(0);
    }
    let outcome = inbox::drain(&cfg.dir, &cfg.backlog)?;
    for line in &outcome.applied {
        println!("{line}");
    }
    for line in &outcome.rejected {
        eprintln!("rejected: {line}");
    }
    // A drain applies the whole queue, so report only on OUR request — someone
    // else's stale entry must not fail this call.
    let ours = queued.file_name().unwrap_or_default().to_string_lossy();
    Ok(outcome.rejected.iter().any(|r| r.starts_with(&*ours)) as i32)
}

pub fn run(sub: &str, argv: &[String]) -> R<i32> {
    let args = parse(argv)?;
    if args.help {
        print!("{USAGE}");
        return Ok(0);
    }
    let cfg = config::load_base(&args.rest)?;
    let text = current(&cfg.backlog)?;
    // Only `add` carries a body, and only it may consume stdin.
    let body = if sub == "add" {
        read_body(args.verify.as_deref())?
    } else {
        String::new()
    };
    match request(sub, &args, &text, body)? {
        Ok(req) => submit(&cfg, &text, req, sub),
        Err(code) => Ok(code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backlog::SCHEMA_MARKER;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn body() -> String {
        "Verify: y".to_string()
    }

    fn backlog() -> String {
        format!("{SCHEMA_MARKER}\n# B\n- [ ] **1 — First.**\n  Verify: y\n  - [ ] **1.1 — Kid.** Verify: y\n")
    }

    #[test]
    fn config_flags_survive_positional_arguments() {
        let a = parse(&argv(&[
            "3.1.1",
            "A title",
            "--verify",
            "cargo test",
            "--dir",
            "/tmp/x",
        ]))
        .unwrap();
        assert_eq!(a.positional, vec!["3.1.1", "A title"]);
        assert_eq!(a.verify.as_deref(), Some("cargo test"));
        assert_eq!(a.rest, vec!["--dir", "/tmp/x"]);
    }

    #[test]
    fn a_missing_flag_value_is_a_usage_error() {
        assert!(parse(&argv(&["title", "--verify"])).is_err());
    }

    #[test]
    fn ids_are_told_from_titles_by_shape() {
        assert!(looks_like_id("3.1.1"));
        assert!(looks_like_id("12"));
        assert!(!looks_like_id("Ship the thing"));
        assert!(!looks_like_id(""));
    }

    #[test]
    fn add_forms_map_to_placements() {
        let text = backlog();
        let explicit = parse(&argv(&["1.2", "Stage", "--verify", "y"])).unwrap();
        assert!(matches!(
            request("add", &explicit, &text, body()).unwrap().unwrap(),
            Request::Add { id: Some(i), .. } if i == "1.2"
        ));
        let under = parse(&argv(&["--under", "1", "Stage", "--verify", "y"])).unwrap();
        assert!(matches!(
            request("add", &under, &text, body()).unwrap().unwrap(),
            Request::Add { id: None, under: Some(p), .. } if p == "1"
        ));
        let plain = parse(&argv(&["Top level", "--verify", "y"])).unwrap();
        assert!(matches!(
            request("add", &plain, &text, body()).unwrap().unwrap(),
            Request::Add {
                id: None,
                under: None,
                ..
            }
        ));
    }

    #[test]
    fn a_duplicate_id_is_a_conflict_not_a_lint_dump() {
        let args = parse(&argv(&["1.1", "Clash", "--verify", "y"])).unwrap();
        assert_eq!(
            request("add", &args, &backlog(), body()).unwrap(),
            Err(EXIT_CONFLICT)
        );
    }

    #[test]
    fn an_id_without_a_title_is_a_usage_error() {
        let args = parse(&argv(&["1.2"])).unwrap();
        assert!(request("add", &args, &backlog(), body()).is_err());
    }

    #[test]
    fn dropping_the_selected_leaf_is_refused_only_while_a_loop_runs() {
        let req = Request::Drop {
            id: "1.1".into(),
            recursive: false,
        };
        assert!(guards_the_selected_leaf(&req, &backlog(), None).is_none());
        assert!(guards_the_selected_leaf(&req, &backlog(), Some(1)).is_some());
        let other = Request::Drop {
            id: "1".into(),
            recursive: true,
        };
        assert!(guards_the_selected_leaf(&other, &backlog(), Some(1)).is_none());
    }
}
