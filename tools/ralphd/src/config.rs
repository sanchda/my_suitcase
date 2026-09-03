//! ralphd configuration: global bot settings (token, guild, user) plus one
//! [`LoopConfig`] per Discord channel — the channel a command arrives in is what
//! selects the loop. A TOML file (`--config`, default `~/.config/ralphd.toml`)
//! is the multi-loop form; the original flags still describe a single loop.
//! The bot token is env-only (`DISCORD_BOT_TOKEN`).

use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct BotConfig {
    pub token: String,
    pub guild_id: u64,
    pub user_id: u64,
    /// Keyed by channel id — the dispatch key for every command and watcher.
    pub loops: HashMap<u64, LoopConfig>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoopConfig {
    pub name: String,
    pub channel_id: u64,
    pub working_dir: PathBuf,
    pub state_dir: PathBuf,
    /// The loop's `ralph.toml`, read at render time for the card's budget line.
    pub ralph_config: PathBuf,
    pub ralph_args: Vec<String>,
    /// Set explicitly on each spawned child: a single inherited `DISCORD_WEBHOOK`
    /// would funnel every loop's lifecycle posts into one channel.
    pub webhook: Option<String>,
    /// Start this loop on connect; otherwise it waits for an explicit `/start`.
    pub autostart: bool,
}

/// The TOML file: globals at the top, then a `[[loop]]` table per channel.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    guild: Option<u64>,
    user: Option<u64>,
    #[serde(rename = "loop", default)]
    loops: Vec<FileLoop>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileLoop {
    name: String,
    channel: u64,
    dir: String,
    #[serde(default)]
    args: Vec<String>,
    webhook: Option<String>,
    #[serde(default)]
    autostart: bool,
}

/// The loop's state dir defaults to `<working-dir>/.ralph`, but a `--dir <path>`
/// in the forwarded ralph args moves it, so ralphd must read the same one ralph
/// writes. A relative `--dir` resolves against working_dir, matching ralph.
pub fn resolve_state_dir(working_dir: &Path, args: &[String]) -> PathBuf {
    match forwarded_path(args, "--dir") {
        Some(dir) if dir.is_absolute() => dir,
        Some(dir) => working_dir.join(dir),
        None => working_dir.join(".ralph"),
    }
}

/// Same resolution for `ralph.toml`, which ralph defaults to `.ralph/ralph.toml`
/// *relative to its cwd* rather than to `--dir`.
pub fn resolve_ralph_config(working_dir: &Path, args: &[String]) -> PathBuf {
    match forwarded_path(args, "--config") {
        Some(p) if p.is_absolute() => p,
        Some(p) => working_dir.join(p),
        None => working_dir.join(".ralph/ralph.toml"),
    }
}

pub fn forwarded_path(args: &[String], flag: &str) -> Option<PathBuf> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
}

/// A truthy env value: anything but empty, `0`, or `false`.
fn env_truthy(v: &str) -> bool {
    let v = v.trim();
    !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
}

/// `argv` excludes the program name; `env` looks a variable up by name so the
/// tests can drive the whole precedence chain without touching the process env.
pub fn parse(argv: &[String], env: impl Fn(&str) -> Option<String>) -> Result<BotConfig, String> {
    // Split at the first bare `--`: before = ralphd flags, after = ralph args.
    let split = argv.iter().position(|a| a == "--");
    let (mine, forwarded) = match split {
        Some(i) => (&argv[..i], argv[i + 1..].to_vec()),
        None => (argv, Vec::new()),
    };

    let flag = |name: &str| -> Option<String> {
        mine.iter()
            .position(|a| a == name)
            .and_then(|i| mine.get(i + 1))
            .cloned()
    };
    let has_flag = |name: &str| mine.iter().any(|a| a == name);

    let token = env("DISCORD_BOT_TOKEN")
        .filter(|t| !t.trim().is_empty())
        .ok_or("DISCORD_BOT_TOKEN is required (env only)")?;

    // An explicit --config wins; then the historical flag form (so today's
    // launch keeps working even next to a stale default file); then the default.
    let explicit = flag("--config").or_else(|| env("RALPHD_CONFIG"));
    let default_path = env("HOME").map(|h| PathBuf::from(h).join(".config/ralphd.toml"));
    let has_channel = flag("--channel").is_some() || env("RALPHD_CHANNEL_ID").is_some();
    let chosen = match (explicit, has_channel, default_path) {
        (Some(p), _, _) => Some(PathBuf::from(p)),
        (None, true, _) => None,
        (None, false, Some(p)) if p.exists() => Some(p),
        _ => None,
    };

    match chosen {
        Some(path) => {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("could not read {}: {e}", path.display()))?;
            from_toml(&text, token, &env)
        }
        None => {
            let want_u64 = |name: &str, env_key: &str| -> Result<u64, String> {
                let raw = flag(name)
                    .or_else(|| env(env_key))
                    .ok_or_else(|| format!("{name} (or {env_key}) is required"))?;
                raw.trim()
                    .parse::<u64>()
                    .map_err(|_| format!("{name} must be a numeric Discord id, got `{raw}`"))
            };
            let guild_id = want_u64("--guild", "RALPHD_GUILD_ID")?;
            let channel_id = want_u64("--channel", "RALPHD_CHANNEL_ID")?;
            let user_id = want_u64("--user", "RALPHD_USER_ID")?;

            let working_dir: PathBuf = flag("--working-dir")
                .or_else(|| env("RALPHD_WORKING_DIR"))
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));

            let autostart = has_flag("--autostart")
                || env("RALPHD_AUTOSTART").map(|v| env_truthy(&v)).unwrap_or(false);

            let lc = LoopConfig {
                name: loop_name(&working_dir),
                channel_id,
                state_dir: resolve_state_dir(&working_dir, &forwarded),
                ralph_config: resolve_ralph_config(&working_dir, &forwarded),
                working_dir,
                ralph_args: forwarded,
                webhook: env("DISCORD_WEBHOOK").filter(|w| !w.trim().is_empty()),
                autostart,
            };
            Ok(BotConfig {
                token,
                guild_id,
                user_id,
                loops: HashMap::from([(channel_id, lc)]),
            })
        }
    }
}

/// Name a flag-form loop after its repo directory, so its card is still labelled.
fn loop_name(working_dir: &Path) -> String {
    working_dir
        .canonicalize()
        .unwrap_or_else(|_| working_dir.to_path_buf())
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ralph".to_string())
}

fn from_toml(
    text: &str,
    token: String,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<BotConfig, String> {
    let file: FileConfig = toml::from_str(text).map_err(|e| format!("bad ralphd config: {e}"))?;

    let want = |from_file: Option<u64>, env_key: &str, name: &str| -> Result<u64, String> {
        match from_file {
            Some(v) => Ok(v),
            None => env(env_key)
                .ok_or_else(|| format!("`{name}` is required in the config (or {env_key})"))?
                .trim()
                .parse::<u64>()
                .map_err(|_| format!("`{name}` must be a numeric Discord id")),
        }
    };
    let guild_id = want(file.guild, "RALPHD_GUILD_ID", "guild")?;
    let user_id = want(file.user, "RALPHD_USER_ID", "user")?;

    if file.loops.is_empty() {
        return Err("the config declares no [[loop]] — ralphd has nothing to drive".into());
    }
    // A single loop may still inherit the ambient webhook (that is how the flag
    // form has always run); with several, an inherited one would cross channels.
    let inherit_webhook = file.loops.len() == 1;

    let mut loops: HashMap<u64, LoopConfig> = HashMap::new();
    for l in file.loops {
        if l.channel == 0 {
            return Err(format!("loop `{}` has no channel id", l.name));
        }
        let working_dir = PathBuf::from(&l.dir);
        let webhook = l
            .webhook
            .or_else(|| {
                inherit_webhook
                    .then(|| env("DISCORD_WEBHOOK"))
                    .flatten()
            })
            .filter(|w| !w.trim().is_empty());
        let lc = LoopConfig {
            name: l.name,
            channel_id: l.channel,
            state_dir: resolve_state_dir(&working_dir, &l.args),
            ralph_config: resolve_ralph_config(&working_dir, &l.args),
            working_dir,
            ralph_args: l.args,
            webhook,
            autostart: l.autostart,
        };
        if let Some(prev) = loops.insert(l.channel, lc) {
            return Err(format!(
                "channel {} is claimed by both `{}` and another loop — one channel drives one loop",
                l.channel, prev.name
            ));
        }
    }
    Ok(BotConfig {
        token,
        guild_id,
        user_id,
        loops,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_map<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    /// The single loop of a flag-form config.
    fn only(cfg: &BotConfig) -> &LoopConfig {
        assert_eq!(cfg.loops.len(), 1, "expected exactly one loop");
        cfg.loops.values().next().unwrap()
    }

    fn tmp_toml(body: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "ralphd-cfg-{}-{}.toml",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn parse_splits_flags_from_forwarded_args() {
        let argv: Vec<String> = [
            "--guild",
            "1",
            "--channel",
            "2",
            "--user",
            "3",
            "--working-dir",
            "/w",
            "--",
            "--model",
            "opus",
            "--dir",
            "custom",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let cfg = parse(&argv, env_map(&[("DISCORD_BOT_TOKEN", "tok")])).unwrap();
        assert_eq!(cfg.guild_id, 1);
        assert_eq!(cfg.user_id, 3);
        let l = only(&cfg);
        assert_eq!(l.channel_id, 2);
        assert_eq!(l.ralph_args, vec!["--model", "opus", "--dir", "custom"]);
        assert_eq!(l.state_dir, PathBuf::from("/w/custom"));
    }

    #[test]
    fn parse_resolves_relative_dir_against_working_dir() {
        let argv: Vec<String> = [
            "--guild", "1", "--channel", "2", "--user", "3", "--working-dir", "/repo", "--",
            "--dir", "custom",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let cfg = parse(&argv, env_map(&[("DISCORD_BOT_TOKEN", "tok")])).unwrap();
        assert_eq!(only(&cfg).state_dir, PathBuf::from("/repo/custom"));
    }

    #[test]
    fn parse_defaults_state_dir_under_working_dir() {
        let argv: Vec<String> = [
            "--guild",
            "1",
            "--channel",
            "2",
            "--user",
            "3",
            "--working-dir",
            "/repo",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let cfg = parse(&argv, env_map(&[("DISCORD_BOT_TOKEN", "tok")])).unwrap();
        let l = only(&cfg);
        assert_eq!(l.state_dir, PathBuf::from("/repo/.ralph"));
        assert_eq!(l.ralph_config, PathBuf::from("/repo/.ralph/ralph.toml"));
        assert!(l.ralph_args.is_empty());
    }

    #[test]
    fn parse_requires_token() {
        let argv: Vec<String> = ["--guild", "1", "--channel", "2", "--user", "3"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(parse(&argv, env_map(&[])).is_err());
    }

    #[test]
    fn autostart_off_by_default_on_with_flag_or_env() {
        let base = ["--guild", "1", "--channel", "2", "--user", "3"];
        let argv: Vec<String> = base.iter().map(|s| s.to_string()).collect();
        let cfg = parse(&argv, env_map(&[("DISCORD_BOT_TOKEN", "tok")])).unwrap();
        assert!(!only(&cfg).autostart);

        let mut with_flag = base.to_vec();
        with_flag.push("--autostart");
        let argv: Vec<String> = with_flag.iter().map(|s| s.to_string()).collect();
        let cfg = parse(&argv, env_map(&[("DISCORD_BOT_TOKEN", "tok")])).unwrap();
        assert!(only(&cfg).autostart);

        let argv: Vec<String> = base.iter().map(|s| s.to_string()).collect();
        let cfg = parse(
            &argv,
            env_map(&[("DISCORD_BOT_TOKEN", "tok"), ("RALPHD_AUTOSTART", "1")]),
        )
        .unwrap();
        assert!(only(&cfg).autostart);
        let cfg = parse(
            &argv,
            env_map(&[("DISCORD_BOT_TOKEN", "tok"), ("RALPHD_AUTOSTART", "false")]),
        )
        .unwrap();
        assert!(!only(&cfg).autostart);
    }

    #[test]
    fn parse_reads_ids_from_env() {
        let argv: Vec<String> = Vec::new();
        let cfg = parse(
            &argv,
            env_map(&[
                ("DISCORD_BOT_TOKEN", "tok"),
                ("RALPHD_GUILD_ID", "10"),
                ("RALPHD_CHANNEL_ID", "20"),
                ("RALPHD_USER_ID", "30"),
            ]),
        )
        .unwrap();
        assert_eq!((cfg.guild_id, cfg.user_id), (10, 30));
        assert_eq!(only(&cfg).channel_id, 20);
    }

    #[test]
    fn flag_form_inherits_the_ambient_webhook() {
        let argv: Vec<String> = ["--guild", "1", "--channel", "2", "--user", "3"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let cfg = parse(
            &argv,
            env_map(&[("DISCORD_BOT_TOKEN", "tok"), ("DISCORD_WEBHOOK", "https://hook")]),
        )
        .unwrap();
        assert_eq!(only(&cfg).webhook.as_deref(), Some("https://hook"));
    }

    #[test]
    fn toml_config_declares_a_loop_per_channel() {
        let path = tmp_toml(
            r#"
guild = 123
user  = 456

[[loop]]
name    = "number-grove"
channel = 111
dir     = "/repos/number_grove"
args    = ["--model", "sonnet"]
webhook = "https://hook/grove"

[[loop]]
name      = "suitcase"
channel   = 222
dir       = "/repos/suitcase"
args      = ["--dir", "/var/ralph/suitcase"]
autostart = true
"#,
        );
        let argv: Vec<String> = ["--config", path.to_str().unwrap()]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let cfg = parse(&argv, env_map(&[("DISCORD_BOT_TOKEN", "tok")])).unwrap();
        assert_eq!((cfg.guild_id, cfg.user_id), (123, 456));
        assert_eq!(cfg.loops.len(), 2);

        let grove = &cfg.loops[&111];
        assert_eq!(grove.name, "number-grove");
        assert_eq!(grove.state_dir, PathBuf::from("/repos/number_grove/.ralph"));
        assert_eq!(grove.ralph_args, vec!["--model", "sonnet"]);
        assert_eq!(grove.webhook.as_deref(), Some("https://hook/grove"));
        assert!(!grove.autostart);

        let suitcase = &cfg.loops[&222];
        assert_eq!(suitcase.state_dir, PathBuf::from("/var/ralph/suitcase"));
        assert!(suitcase.autostart);
    }

    #[test]
    fn several_loops_never_inherit_one_ambient_webhook() {
        let path = tmp_toml(
            "guild = 1\nuser = 2\n\
             [[loop]]\nname = \"a\"\nchannel = 10\ndir = \"/a\"\n\
             [[loop]]\nname = \"b\"\nchannel = 20\ndir = \"/b\"\n",
        );
        let argv: Vec<String> = ["--config", path.to_str().unwrap()]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let cfg = parse(
            &argv,
            env_map(&[("DISCORD_BOT_TOKEN", "tok"), ("DISCORD_WEBHOOK", "https://one")]),
        )
        .unwrap();
        assert!(cfg.loops.values().all(|l| l.webhook.is_none()));

        // A lone loop still inherits it, so the historical launch is unchanged.
        let solo = tmp_toml("guild = 1\nuser = 2\n[[loop]]\nname = \"a\"\nchannel = 10\ndir = \"/a\"\n");
        let argv: Vec<String> = ["--config", solo.to_str().unwrap()]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let cfg = parse(
            &argv,
            env_map(&[("DISCORD_BOT_TOKEN", "tok"), ("DISCORD_WEBHOOK", "https://one")]),
        )
        .unwrap();
        assert_eq!(cfg.loops[&10].webhook.as_deref(), Some("https://one"));
    }

    #[test]
    fn toml_rejects_duplicate_channels_and_empty_loop_lists() {
        let dup = tmp_toml(
            "guild = 1\nuser = 2\n\
             [[loop]]\nname = \"a\"\nchannel = 10\ndir = \"/a\"\n\
             [[loop]]\nname = \"b\"\nchannel = 10\ndir = \"/b\"\n",
        );
        let argv: Vec<String> = ["--config", dup.to_str().unwrap()]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let err = parse(&argv, env_map(&[("DISCORD_BOT_TOKEN", "tok")])).unwrap_err();
        assert!(err.contains("one channel drives one loop"), "{err}");

        let empty = tmp_toml("guild = 1\nuser = 2\n");
        let argv: Vec<String> = ["--config", empty.to_str().unwrap()]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(parse(&argv, env_map(&[("DISCORD_BOT_TOKEN", "tok")]))
            .unwrap_err()
            .contains("no [[loop]]"));
    }

    #[test]
    fn explicit_config_flag_wins_over_the_flag_form() {
        let path = tmp_toml("guild = 9\nuser = 8\n[[loop]]\nname = \"a\"\nchannel = 77\ndir = \"/a\"\n");
        let argv: Vec<String> = [
            "--config",
            path.to_str().unwrap(),
            "--guild",
            "1",
            "--channel",
            "2",
            "--user",
            "3",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let cfg = parse(&argv, env_map(&[("DISCORD_BOT_TOKEN", "tok")])).unwrap();
        assert_eq!(cfg.guild_id, 9);
        assert!(cfg.loops.contains_key(&77));
    }

    #[test]
    fn missing_config_file_is_an_error_not_a_silent_fallback() {
        let argv: Vec<String> = ["--config", "/nonexistent/ralphd.toml"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(parse(&argv, env_map(&[("DISCORD_BOT_TOKEN", "tok")]))
            .unwrap_err()
            .contains("could not read"));
    }

    /// `deny_unknown_fields` means a drifted example is a hard error for whoever
    /// copies it, and nothing else would catch that.
    #[test]
    fn the_committed_example_config_still_parses() {
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("ralphd.toml.example");
        let argv: Vec<String> = ["--config", example.to_str().unwrap()]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let cfg = parse(&argv, env_map(&[("DISCORD_BOT_TOKEN", "tok")]))
            .unwrap_or_else(|e| panic!("{} does not parse: {e}", example.display()));
        assert!(
            cfg.loops.len() >= 2,
            "the example must keep showing the multi-loop shape"
        );
    }
}
