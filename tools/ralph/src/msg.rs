//! `ralph msg` — a persistent steering session attached to this repo's loop.
//!
//! The session id lives in `<dir>/msg-session`, so a follow-up like "no, do it
//! the other way" lands in the same context instead of re-establishing it. The
//! session steers the loop through this same CLI and carries no tools of its own.

use crate::{config, pidguard, R};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const USAGE: &str = "\
Usage: ralph msg [--new] [--model <m>] [--stream-json] [--] <text>
       ralph msg --new            Reset the session without sending anything

Send <text> to this repo's steering session, resuming it (or creating it on
first use). Only one msg runs at a time; a second is refused, not queued.

  --new            Archive the current session id and start a fresh one
  --model <m>      Repin the session's model; sticks until changed or --new
  --stream-json    Pass claude's raw NDJSON events through on stdout
  --dir <path>     Runtime dir (default .ralph)
  --config <file>  Config file (default .ralph/ralph.toml)

--model takes anything claude takes (an alias like `opus`, or a full name like
`claude-fable-5`); it is not checked against the loop's escalation ladder.
";

/// The session's whole job is to drive the loop through the CLI, so it needs no
/// tool definitions — a shell and this map are the entire control surface.
const PREAMBLE: &str = "\
You are attached to the ralph loop running in this repo, via `ralph msg`.

You STEER the loop; you do not do the work yourself. Read, decide, adjust the
queue, reply briefly. Leave implementation to the loop's own iterations — that
is what keeps this session cheap, and it is usually read from a phone.

- `ralph status` prints the backlog frontier (`--json` for structure).
- `.ralph/live` is the iteration running right now; `.ralph/run.log` is history.
- Queue work with `ralph add <title>`, or `ralph add --under <parent> <title>`
  for a child stage. NEVER edit BACKLOG.md by hand: the running loop owns that
  file and a hand edit is silently overwritten.
- `ralph done <id>`, `ralph uncheck <id>`, `ralph drop <id>` are the other
  schema-checked mutations.
- `ralph model <tier>` retiers the next pass; `ralph stop` halts after the
  current task (`ralph stop --now` signals the running turn as well).
";

/// Where the current session id is recorded.
pub fn session_path(dir: &Path) -> PathBuf {
    dir.join("msg-session")
}

/// The session's model sticks until changed: a conversation has one model in the
/// operator's head, and silently reverting mid-thread is the worse surprise.
fn model_path(dir: &Path) -> PathBuf {
    dir.join("msg-model")
}

fn pidfile(dir: &Path) -> PathBuf {
    dir.join("msg.pid")
}

/// The session's pinned model, if one was set. Not validated against the
/// escalation ladder — claude accepts aliases and full names alike, and this
/// never reaches the loop's own tier machinery.
fn read_model(dir: &Path) -> Option<String> {
    let m = std::fs::read_to_string(model_path(dir))
        .ok()?
        .trim()
        .to_string();
    (!m.is_empty()).then_some(m)
}

/// `--session-id`/`--resume` demand a real UUID, so a truncated or hand-mangled
/// `msg-session` must never reach claude.
fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// The recorded session id, if one is present and usable.
fn read_session(dir: &Path) -> Option<String> {
    let id = std::fs::read_to_string(session_path(dir))
        .ok()?
        .trim()
        .to_string();
    is_uuid(&id).then_some(id)
}

/// A fresh session id from the kernel, which keeps `ralph` free of a uuid crate.
fn new_uuid() -> R<String> {
    const SOURCE: &str = "/proc/sys/kernel/random/uuid";
    let id = std::fs::read_to_string(SOURCE)
        .map_err(|e| format!("reading {SOURCE}: {e}"))?
        .trim()
        .to_string();
    if !is_uuid(&id) {
        return Err(format!("{SOURCE} returned a non-UUID session id: {id}").into());
    }
    Ok(id)
}

/// Retire the current session id into `<dir>/archive/`. Context accretes until it
/// is expensive and then until it does not fit, so both `--new` and a completed
/// arc reset it. Returns the archived id, if there was one.
pub fn archive_session(dir: &Path) -> Option<String> {
    let path = session_path(dir);
    let id = read_session(dir);
    let archive = dir.join("archive");
    if id.is_some() && std::fs::create_dir_all(&archive).is_ok() {
        let dest = archive.join(format!("msg-session-{}", crate::state::timestamp()));
        let _ = std::fs::rename(&path, &dest);
    }
    let _ = std::fs::remove_file(&path);
    // The pinned model belongs to the retired conversation, not the next one.
    let _ = std::fs::remove_file(model_path(dir));
    id
}

/// Take the single-message guard, naming the live holder on refusal.
fn guard(dir: &Path) -> Result<pidguard::Guard, String> {
    let path = pidfile(dir);
    pidguard::acquire(&path).map_err(|held| match held {
        Some(pid) => format!("a msg session is already running (pid {pid}) — refused, not queued"),
        None => format!("could not take {}", path.display()),
    })
}

/// The exact `claude` argv. `resume` distinguishes continuing a session from
/// establishing the id we just generated.
fn claude_args(id: &str, resume: bool, preamble: &str, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        // Matches how the loop itself runs claude; a steering session that stops
        // to ask for permission would hang with nobody to answer it.
        "--dangerously-skip-permissions".into(),
        "--append-system-prompt".into(),
        preamble.into(),
    ];
    // Session-scoped, so it applies to a resumed conversation from here on.
    if let Some(m) = model {
        args.push("--model".into());
        args.push(m.into());
    }
    args.push(if resume { "--resume" } else { "--session-id" }.into());
    args.push(id.into());
    args
}

/// The final text of a `{"type":"result"}` envelope, if this line is one.
fn result_text(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("result") {
        return None;
    }
    Some(
        v.get("result")
            .and_then(|r| r.as_str())
            .unwrap_or_default()
            .to_string(),
    )
}

/// Run one message. In `--stream-json` mode claude's stdout is inherited, so the
/// NDJSON reaches the caller line-by-line unbuffered and unreformatted (ralphd
/// folds it); otherwise we read it and print only the final text.
fn send(id: &str, resume: bool, text: &str, stream_json: bool, model: Option<&str>) -> R<i32> {
    let mut cmd = Command::new("claude");
    cmd.args(claude_args(id, resume, PREAMBLE, model))
        .stdin(Stdio::piped())
        .stdout(if stream_json {
            Stdio::inherit()
        } else {
            Stdio::piped()
        });
    let mut child = cmd.spawn()?;

    // Feed the message on its own thread so a long one can't deadlock against
    // the stream we're reading.
    let mut stdin = child.stdin.take().expect("piped stdin");
    let body = text.to_string();
    let writer = std::thread::spawn(move || stdin.write_all(body.as_bytes()));

    let mut final_text = None;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(t) = result_text(&line) {
                final_text = Some(t);
            }
        }
    }
    let status = child.wait()?;
    let _ = writer.join();

    let code = status.code().unwrap_or(1);
    match final_text {
        Some(t) => println!("{t}"),
        None if !stream_json => eprintln!("ralph: no result envelope (claude exited {code})"),
        None => {}
    }
    Ok(code)
}

/// Parsed `ralph msg` argv.
#[derive(Debug, Default, PartialEq)]
struct Args {
    new: bool,
    stream_json: bool,
    help: bool,
    dir: Option<PathBuf>,
    /// Repins the session's model; absent leaves the stored pin alone.
    model: Option<String>,
    /// Forwarded to config resolution (`--config <file>`).
    passthrough: Vec<String>,
    text: String,
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut args = Args::default();
    let mut words: Vec<String> = Vec::new();
    let mut it = argv.iter();
    let mut flags_done = false;
    while let Some(a) = it.next() {
        let mut next = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            _ if flags_done => words.push(a.clone()),
            "--" => flags_done = true,
            "--new" => args.new = true,
            "--stream-json" => args.stream_json = true,
            "-h" | "--help" => args.help = true,
            "--dir" => args.dir = Some(PathBuf::from(next()?)),
            "--model" => args.model = Some(next()?),
            "--config" => {
                args.passthrough.push(a.clone());
                args.passthrough.push(next()?);
            }
            other if other.starts_with('-') => return Err(format!("unknown arg: {other}")),
            other => words.push(other.to_string()),
        }
    }
    args.text = words.join(" ");
    Ok(args)
}

pub fn run(argv: &[String]) -> R<i32> {
    let args = parse(argv)?;
    if args.help {
        print!("{USAGE}");
        return Ok(0);
    }
    if args.text.trim().is_empty() && !args.new {
        return Err("usage: ralph msg [--new] [--stream-json] <text>".into());
    }

    let mut cfg = config::load_base(&args.passthrough)?;
    if let Some(dir) = args.dir {
        cfg.dir = dir;
    }
    std::fs::create_dir_all(&cfg.dir)?;

    let _guard = match guard(&cfg.dir) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("ralph: {e}");
            return Ok(1);
        }
    };

    if args.new {
        // Diagnostics go to stderr: stdout may be a raw NDJSON stream ralphd folds.
        match archive_session(&cfg.dir) {
            Some(old) => eprintln!("ralph: archived session {old}"),
            None => eprintln!("ralph: no session to archive"),
        }
    }
    // `ralph msg --new` on its own is a reset with nothing to say.
    if args.text.trim().is_empty() {
        return Ok(0);
    }

    let existing = read_session(&cfg.dir);
    let id = match &existing {
        Some(id) => id.clone(),
        None => new_uuid()?,
    };

    // An explicit --model repins; otherwise the stored pin carries the thread.
    let model = args.model.clone().or_else(|| read_model(&cfg.dir));
    if let Some(m) = &model {
        eprintln!("ralph: session {id} on {m}");
    }

    let code = send(
        &id,
        existing.is_some(),
        &args.text,
        args.stream_json,
        model.as_deref(),
    )?;
    // Record a new id only once claude has established it, or every later
    // --resume would fail against a session that never existed. A repin is
    // persisted on the same terms, so a rejected model name can't brick the
    // session the way a phantom id would.
    if code == 0 {
        if existing.is_none() {
            std::fs::write(session_path(&cfg.dir), format!("{id}\n"))?;
        }
        if let Some(m) = args.model {
            std::fs::write(model_path(&cfg.dir), format!("{m}\n"))?;
        }
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmp() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ralph-msg-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_flags_and_joins_the_message() {
        let a = parse(&argv(&["--new", "try", "the", "other", "way"])).unwrap();
        assert!(a.new && !a.stream_json);
        assert_eq!(a.text, "try the other way");

        let a = parse(&argv(&["--stream-json", "--config", "c.toml", "hi"])).unwrap();
        assert!(a.stream_json);
        assert_eq!(a.passthrough, vec!["--config".to_string(), "c.toml".into()]);
        assert_eq!(a.text, "hi");

        let a = parse(&argv(&["--dir", "/tmp/x", "hi"])).unwrap();
        assert_eq!(a.dir, Some(PathBuf::from("/tmp/x")));
    }

    #[test]
    fn a_dash_leading_message_needs_the_separator() {
        assert!(parse(&argv(&["--nope", "hi"])).is_err());
        assert_eq!(
            parse(&argv(&["--", "--nope", "hi"])).unwrap().text,
            "--nope hi"
        );
        assert!(parse(&argv(&["--dir"])).is_err());
    }

    #[test]
    fn help_is_recognized_anywhere() {
        assert!(parse(&argv(&["--help"])).unwrap().help);
        assert!(parse(&argv(&["hi", "-h"])).unwrap().help);
    }

    const ID: &str = "11111111-2222-3333-4444-555555555555";

    #[test]
    fn first_call_creates_the_session_later_calls_resume_it() {
        let create = claude_args(ID, false, "P", None);
        assert!(create.contains(&"--session-id".to_string()));
        assert!(!create.contains(&"--resume".to_string()));

        let resume = claude_args(ID, true, "P", None);
        assert!(resume.contains(&"--resume".to_string()));
        assert!(!resume.contains(&"--session-id".to_string()));
        assert_eq!(resume.last().unwrap(), ID);
    }

    #[test]
    fn argv_carries_the_stream_and_permission_flags() {
        let a = claude_args(ID, false, "PRE", None);
        assert_eq!(a[0], "-p");
        assert!(a
            .windows(2)
            .any(|w| w == ["--output-format", "stream-json"]));
        assert!(a.contains(&"--verbose".to_string()));
        assert!(a.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(a.windows(2).any(|w| w == ["--append-system-prompt", "PRE"]));
        assert!(!a.contains(&"--model".to_string()));
    }

    #[test]
    fn a_pinned_model_reaches_claude_without_displacing_the_session_id() {
        let a = claude_args(ID, true, "P", Some("opus"));
        assert!(a.windows(2).any(|w| w == ["--model", "opus"]));
        // The id must stay the trailing value of --resume, not of --model.
        assert!(a.windows(2).any(|w| w == ["--resume", ID]));
        assert_eq!(a.last().unwrap(), ID);
    }

    #[test]
    fn the_model_pin_is_read_back_and_retired_with_the_session() {
        let dir = tmp();
        assert_eq!(read_model(&dir), None);
        std::fs::write(model_path(&dir), "opus\n").unwrap();
        assert_eq!(read_model(&dir), Some("opus".into()));
        // Whitespace-only is as good as absent.
        std::fs::write(model_path(&dir), "  \n").unwrap();
        assert_eq!(read_model(&dir), None);

        std::fs::write(model_path(&dir), "opus\n").unwrap();
        std::fs::write(session_path(&dir), format!("{ID}\n")).unwrap();
        archive_session(&dir);
        assert_eq!(read_model(&dir), None, "pin belongs to the retired thread");
    }

    #[test]
    fn model_parses_and_is_optional() {
        let a = parse(&argv(&["--model", "opus", "hi"])).unwrap();
        assert_eq!(a.model.as_deref(), Some("opus"));
        assert_eq!(a.text, "hi");
        assert_eq!(parse(&argv(&["hi"])).unwrap().model, None);
        assert!(parse(&argv(&["--model"])).is_err());
    }

    #[test]
    fn the_preamble_names_the_control_surface() {
        for command in ["ralph status", "ralph add", "ralph model", "ralph stop"] {
            assert!(PREAMBLE.contains(command), "missing {command}");
        }
        assert!(PREAMBLE.contains("NEVER edit BACKLOG.md"));
    }

    #[test]
    fn only_a_real_uuid_is_accepted() {
        assert!(is_uuid("11111111-2222-3333-4444-555555555555"));
        assert!(!is_uuid("1111111-2222-3333-4444-555555555555"));
        assert!(!is_uuid("11111111x2222-3333-4444-555555555555"));
        assert!(!is_uuid("gggggggg-2222-3333-4444-555555555555"));
        assert!(is_uuid(&new_uuid().unwrap()));
    }

    #[test]
    fn a_corrupt_session_file_reads_as_absent() {
        let dir = tmp();
        assert_eq!(read_session(&dir), None);
        std::fs::write(session_path(&dir), "not a uuid\n").unwrap();
        assert_eq!(read_session(&dir), None);
        let id = new_uuid().unwrap();
        std::fs::write(session_path(&dir), format!("{id}\n")).unwrap();
        assert_eq!(read_session(&dir), Some(id));
    }

    #[test]
    fn archiving_retires_the_id_and_is_a_no_op_when_absent() {
        let dir = tmp();
        assert_eq!(archive_session(&dir), None);
        let id = new_uuid().unwrap();
        std::fs::write(session_path(&dir), format!("{id}\n")).unwrap();
        assert_eq!(archive_session(&dir), Some(id.clone()));
        assert!(!session_path(&dir).exists());
        let archived: Vec<_> = std::fs::read_dir(dir.join("archive"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| std::fs::read_to_string(e.path()).unwrap())
            .collect();
        assert_eq!(archived, vec![format!("{id}\n")]);
    }

    #[test]
    fn a_second_message_is_refused_naming_the_holder() {
        let dir = tmp();
        let Ok(held) = guard(&dir) else {
            panic!("first guard refused")
        };
        let refused = guard(&dir).err().expect("second guard refused");
        assert!(
            refused.contains(&format!("pid {}", std::process::id())),
            "{refused}"
        );
        drop(held);
        assert!(guard(&dir).is_ok());
    }

    #[test]
    fn new_with_no_text_resets_without_spawning_claude() {
        let dir = tmp();
        let id = new_uuid().unwrap();
        std::fs::write(session_path(&dir), format!("{id}\n")).unwrap();
        let argv = argv(&["--new", "--dir", dir.to_str().unwrap()]);
        assert_eq!(run(&argv).unwrap(), 0);
        assert!(!session_path(&dir).exists());
        assert!(!pidfile(&dir).exists()); // guard released
    }

    #[test]
    fn the_result_envelope_is_the_final_text() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"queued 3.1","total_cost_usd":0.01}"#;
        assert_eq!(result_text(line).unwrap(), "queued 3.1");
        assert_eq!(result_text(r#"{"type":"assistant","message":{}}"#), None);
        assert_eq!(result_text("not json"), None);
        // A result with no text still terminates the stream.
        assert_eq!(result_text(r#"{"type":"result"}"#).unwrap(), "");
    }
}
