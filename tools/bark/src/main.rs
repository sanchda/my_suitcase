//! bark -- post an ID-tagged line to a Discord webhook.
//!
//! Silent on success (except `--wait`, which prints the created message id);
//! errors go to stderr. Exit codes: 0 sent, 1 not delivered, 2 bad usage/config.

mod config;
mod post;

use std::io::Read;
use std::path::PathBuf;

const USAGE: &str = "\
bark -- post an ID-tagged line to a Discord webhook

Usage: bark [options] <id> <message...>
       bark [options] --id <id> <message...>
       bark [options] <id> -            Read the message from stdin
       bark init [options]              Write a starter config
       bark targets [options]           List configured targets (tokens redacted)

Options:
  --id <id>            Message id (alternative to the first positional)
  --to <name>          Named target from the config
  --webhook <url>      Post here, ignoring the config
  --username <name>    Display name for this post
  --config <file>      Config file to use
  --timeout <secs>     Per-attempt HTTP timeout (default 10)
  --wait               Wait for Discord and print the created message id
  --dry-run            Print the resolved target and message; post nothing
  --force              init only: overwrite an existing config
  -h, --help           This help
  -V, --version        Print version

Posted as: `[<id>]` <message>   (clamped to Discord's 2000 chars)
Use `--` before a message that starts with a dash.

Config file, first that is set:
  $BARK_CONFIG, $XDG_CONFIG_HOME/bark/config.toml, ~/.config/bark/config.toml

  webhook  = \"https://discord.com/api/webhooks/<id>/<token>\"  # single target
  username = \"bark\"                                          # optional
  default  = \"ops\"                                           # if several targets
  [targets.ops]
  webhook  = \"https://discord.com/api/webhooks/<id>/<token>\"
  username = \"pager\"                                         # optional

Webhook precedence: --webhook, --to <name>, $BARK_WEBHOOK, the config's default.
A 429 or 5xx is retried (honoring Discord's retry_after); a 4xx is not.

Examples:
  bark deploy-42 'gateway rollout finished'
  bark --to alerts oncall 'disk 91% on relay-7'
  make build 2>&1 | tail -5 | bark build-$(git rev-parse --short HEAD) -
";

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, PartialEq)]
enum Cmd {
    Help,
    Version,
    Init(Opts),
    Targets(Opts),
    Send(Send),
}

#[derive(Debug, Default, PartialEq)]
struct Opts {
    config: Option<PathBuf>,
    webhook: Option<String>,
    to: Option<String>,
    username: Option<String>,
    force: bool,
}

#[derive(Debug, PartialEq)]
struct Send {
    id: String,
    /// `None` means read the message from stdin.
    message: Option<String>,
    opts: Opts,
    timeout: u64,
    wait: bool,
    dry_run: bool,
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let code = match run(&argv) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("bark: {e}");
            2
        }
    };
    std::process::exit(code);
}

fn run(argv: &[String]) -> Result<i32, String> {
    match parse(argv)? {
        Cmd::Help => {
            print!("{USAGE}");
            Ok(0)
        }
        Cmd::Version => {
            println!("bark {VERSION}");
            Ok(0)
        }
        Cmd::Init(opts) => init(&opts),
        Cmd::Targets(opts) => targets(&opts),
        Cmd::Send(send) => deliver(send),
    }
}

fn parse(argv: &[String]) -> Result<Cmd, String> {
    let mut opts = Opts::default();
    let mut id: Option<String> = None;
    let mut timeout: u64 = 10;
    let mut wait = false;
    let mut dry_run = false;
    let mut words: Vec<String> = Vec::new();
    let mut subcommand: Option<String> = None;
    let mut only_positionals = false;

    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (arg, None),
        };
        let mut value = |flag: &str| -> Result<String, String> {
            if let Some(v) = inline.clone() {
                return Ok(v);
            }
            i += 1;
            argv.get(i)
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };

        // A bare `-` is the stdin marker; anything else leading with `-` is a
        // flag until `--` says otherwise.
        if only_positionals || arg == "-" || !arg.starts_with('-') {
            let word = arg.to_string();
            let first = words.is_empty() && id.is_none() && subcommand.is_none();
            if first && (word == "init" || word == "targets") {
                subcommand = Some(word);
            } else {
                words.push(word);
            }
            i += 1;
            continue;
        }

        match name {
            "--" => only_positionals = true,
            "--help" | "-h" => return Ok(Cmd::Help),
            "--version" | "-V" => return Ok(Cmd::Version),
            "--wait" => wait = true,
            "--dry-run" => dry_run = true,
            "--force" => opts.force = true,
            "--id" => id = Some(value("--id")?),
            "--to" => opts.to = Some(value("--to")?),
            "--webhook" => opts.webhook = Some(value("--webhook")?),
            "--username" => opts.username = Some(value("--username")?),
            "--config" => opts.config = Some(PathBuf::from(value("--config")?)),
            "--timeout" => {
                let raw = value("--timeout")?;
                timeout = raw
                    .trim()
                    .parse()
                    .map_err(|_| format!("--timeout wants whole seconds, got `{raw}`"))?;
                if timeout == 0 {
                    return Err("--timeout must be at least 1 second".into());
                }
            }
            other => return Err(format!("unknown option `{other}` (see --help)")),
        }
        i += 1;
    }

    if let Some(sub) = subcommand {
        if let Some(extra) = words.first() {
            return Err(format!(
                "`bark {sub}` takes no positional arguments (got `{extra}`)"
            ));
        }
        return Ok(if sub == "init" {
            Cmd::Init(opts)
        } else {
            Cmd::Targets(opts)
        });
    }

    let id = match id {
        Some(id) => id,
        None if words.is_empty() => {
            return Err("nothing to say (usage: bark <id> <message...>)".into())
        }
        None => words.remove(0),
    };
    if id.trim().is_empty() {
        return Err("the message id cannot be blank".into());
    }
    if words.is_empty() {
        return Err(format!(
            "no message for id `{id}` (pass text, or `-` to read stdin)"
        ));
    }

    let message = if words.len() == 1 && words[0] == "-" {
        None
    } else {
        Some(words.join(" "))
    };
    Ok(Cmd::Send(Send {
        id,
        message,
        opts,
        timeout,
        wait,
        dry_run,
    }))
}

fn config_path(opts: &Opts) -> PathBuf {
    config::path(opts.config.clone(), |k| std::env::var(k).ok())
}

fn env_webhook() -> Option<String> {
    std::env::var("BARK_WEBHOOK")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Precedence: `--webhook`, `--to <name>`, `$BARK_WEBHOOK`, config default.
fn destination(opts: &Opts) -> Result<config::Resolved, String> {
    if let Some(url) = opts.webhook.as_deref().map(str::trim) {
        if url.is_empty() {
            return Err("--webhook was empty".into());
        }
        return Ok(config::Resolved {
            name: "--webhook".into(),
            webhook: url.to_string(),
            username: opts.username.clone(),
        });
    }

    let path = config_path(opts);
    let file = config::load(&path)?;

    if opts.to.is_none() {
        if let Some(url) = env_webhook() {
            return Ok(config::Resolved {
                name: "$BARK_WEBHOOK".into(),
                webhook: url,
                username: opts.username.clone().or(file.username),
            });
        }
    }

    let mut resolved = config::resolve(&file, opts.to.as_deref(), &path)?;
    if let Some(name) = opts.username.clone() {
        resolved.username = Some(name);
    }
    Ok(resolved)
}

fn deliver(send: Send) -> Result<i32, String> {
    let target = destination(&send.opts)?;
    let body = match send.message {
        Some(text) => text,
        None => read_stdin()?,
    };
    let content = post::render(&send.id, &body);
    if content.trim().is_empty() {
        return Err("refusing to post an empty message".into());
    }

    let request = post::Post {
        webhook: target.webhook,
        content,
        username: target.username,
        wait: send.wait,
        timeout: send.timeout,
    };

    if send.dry_run {
        println!(
            "target:  {} ({})",
            target.name,
            post::redact(&request.webhook)
        );
        if let Some(name) = request.username.as_deref() {
            println!("as:      {name}");
        }
        println!("content: {}", request.content);
        return Ok(0);
    }

    match post::send(&request) {
        Ok(id) => {
            if let Some(id) = id {
                println!("{id}");
            }
            Ok(0)
        }
        Err(e) => {
            eprintln!("bark: {e}");
            Ok(1)
        }
    }
}

fn read_stdin() -> Result<String, String> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| format!("cannot read stdin: {e}"))?;
    if buf.trim().is_empty() {
        return Err("stdin was empty".into());
    }
    Ok(buf)
}

fn init(opts: &Opts) -> Result<i32, String> {
    let path = config_path(opts);
    if path.exists() && !opts.force {
        return Err(format!(
            "{} already exists (pass --force to overwrite)",
            path.display()
        ));
    }
    let url = opts
        .webhook
        .clone()
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .or_else(env_webhook)
        .ok_or("nothing to write: pass --webhook <url> or set $BARK_WEBHOOK")?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, config::template(&url))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    restrict(&path);
    println!("wrote {}", path.display());
    Ok(0)
}

/// The config holds a webhook token, so keep it owner-only. Best effort.
#[cfg(unix)]
fn restrict(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &std::path::Path) {}

fn targets(opts: &Opts) -> Result<i32, String> {
    let path = config_path(opts);
    let file = config::load(&path)?;
    let names = config::names(&file);
    println!("{}", path.display());
    if names.is_empty() {
        println!("(no targets -- run `bark init --webhook <url>`)");
        if let Some(url) = env_webhook() {
            println!(
                "$BARK_WEBHOOK is set and would be used: {}",
                post::redact(&url)
            );
        }
        return Ok(0);
    }
    let default = config::resolve(&file, None, &path).ok().map(|r| r.name);
    for name in names {
        let marker = if Some(&name) == default.as_ref() {
            "*"
        } else {
            " "
        };
        let shown = config::resolve(&file, Some(&name), &path)
            .map(|r| post::redact(&r.webhook))
            .unwrap_or_else(|e| format!("<{e}>"));
        println!("{marker} {name}  {shown}");
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    fn send_of(args: &[&str]) -> Send {
        match parse(&argv(args)).unwrap() {
            Cmd::Send(s) => s,
            other => panic!("expected a send, got {other:?}"),
        }
    }

    #[test]
    fn positional_id_then_message() {
        let send = send_of(&["build-42", "all", "green"]);
        assert_eq!(send.id, "build-42");
        assert_eq!(send.message.as_deref(), Some("all green"));
        assert_eq!(send.timeout, 10);
        assert!(!send.wait && !send.dry_run);
    }

    #[test]
    fn id_flag_keeps_every_positional_in_the_message() {
        let send = send_of(&["--id", "x", "init", "the", "thing"]);
        assert_eq!(send.id, "x");
        assert_eq!(send.message.as_deref(), Some("init the thing"));
    }

    #[test]
    fn lone_dash_means_stdin() {
        assert_eq!(send_of(&["id", "-"]).message, None);
        assert_eq!(send_of(&["id", "-", "x"]).message.as_deref(), Some("- x"));
    }

    #[test]
    fn flags_accept_both_separated_and_inline_values() {
        let send = send_of(&["--to=alerts", "--timeout=3", "id", "msg"]);
        assert_eq!(send.opts.to.as_deref(), Some("alerts"));
        assert_eq!(send.timeout, 3);
        let send = send_of(&["--to", "alerts", "--timeout", "3", "id", "msg"]);
        assert_eq!(send.opts.to.as_deref(), Some("alerts"));
        assert_eq!(send.timeout, 3);
    }

    #[test]
    fn flags_may_follow_the_message() {
        let send = send_of(&["id", "hello", "--wait", "--dry-run"]);
        assert_eq!(send.message.as_deref(), Some("hello"));
        assert!(send.wait && send.dry_run);
    }

    #[test]
    fn double_dash_protects_leading_dashes() {
        let send = send_of(&["id", "--", "--not-a-flag"]);
        assert_eq!(send.message.as_deref(), Some("--not-a-flag"));
    }

    #[test]
    fn subcommands_only_count_in_first_position() {
        assert_eq!(
            parse(&argv(&["init", "--webhook", "u"])).unwrap(),
            Cmd::Init(Opts {
                webhook: Some("u".into()),
                ..Opts::default()
            })
        );
        assert_eq!(
            parse(&argv(&["targets"])).unwrap(),
            Cmd::Targets(Opts::default())
        );
        assert_eq!(
            send_of(&["id", "targets"]).message.as_deref(),
            Some("targets")
        );
    }

    #[test]
    fn help_and_version_win_over_everything() {
        assert_eq!(parse(&argv(&["id", "msg", "--help"])).unwrap(), Cmd::Help);
        assert_eq!(parse(&argv(&["-V"])).unwrap(), Cmd::Version);
    }

    #[test]
    fn usage_errors_are_specific() {
        let err = |args: &[&str]| parse(&argv(args)).unwrap_err();
        assert!(err(&[]).contains("nothing to say"));
        assert!(err(&["id"]).contains("no message for id `id`"));
        assert!(err(&["  ", "hi"]).contains("cannot be blank"));
        assert!(err(&["--to"]).contains("--to needs a value"));
        assert!(err(&["--nope", "id", "hi"]).contains("unknown option `--nope`"));
        assert!(err(&["--timeout", "soon", "id", "hi"]).contains("whole seconds"));
        assert!(err(&["--timeout", "0", "id", "hi"]).contains("at least 1 second"));
        assert!(err(&["init", "extra"]).contains("no positional arguments"));
    }

    #[test]
    fn help_documents_the_config() {
        assert!(USAGE.contains("$XDG_CONFIG_HOME/bark/config.toml"));
        assert!(USAGE.contains("[targets.ops]"));
        assert!(USAGE.contains("Webhook precedence"));
    }

    #[test]
    fn webhook_flag_bypasses_the_config() {
        let opts = Opts {
            webhook: Some("https://example/hook".into()),
            config: Some(PathBuf::from("/nonexistent/config.toml")),
            ..Opts::default()
        };
        let got = destination(&opts).unwrap();
        assert_eq!(got.webhook, "https://example/hook");
        assert_eq!(got.name, "--webhook");
    }
}
