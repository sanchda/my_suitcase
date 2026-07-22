//! ralphd configuration: its own settings come from flags before `--` (with
//! env fallbacks); everything after `--` is the fixed loop profile forwarded to
//! `ralph` verbatim on `/start`. The bot token is env-only (`DISCORD_BOT_TOKEN`).

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct BotConfig {
    pub token: String,
    pub guild_id: u64,
    pub channel_id: u64,
    pub user_id: u64,
    pub working_dir: PathBuf,
    pub state_dir: PathBuf,
    pub ralph_args: Vec<String>,
    /// Start the loop automatically once connected (opt-in). Otherwise the loop
    /// only starts on an explicit `/start`.
    pub autostart: bool,
}

/// Parse `(argv, env)` into a `BotConfig`. `argv` excludes the program name.
/// `env` looks up an environment variable by name.
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

    // The loop's state dir defaults to `<working-dir>/.ralph`, but honor a
    // `--dir <path>` in the forwarded ralph args so /model writes the right file.
    let dir_override = forwarded
        .iter()
        .position(|a| a == "--dir")
        .and_then(|i| forwarded.get(i + 1))
        .map(PathBuf::from);
    let state_dir = match dir_override {
        Some(dir) if dir.is_absolute() => dir,
        Some(dir) => working_dir.join(dir), // relative --dir resolves against working_dir, matching ralph
        None => working_dir.join(".ralph"),
    };

    // Opt-in: a bare `--autostart` flag, or a truthy `RALPHD_AUTOSTART` env.
    let autostart = has_flag("--autostart")
        || env("RALPHD_AUTOSTART")
            .map(|v| v.trim() != "0" && !v.trim().eq_ignore_ascii_case("false") && !v.trim().is_empty())
            .unwrap_or(false);

    Ok(BotConfig {
        token,
        guild_id,
        channel_id,
        user_id,
        working_dir,
        state_dir,
        ralph_args: forwarded,
        autostart,
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
        assert_eq!(cfg.channel_id, 2);
        assert_eq!(cfg.user_id, 3);
        assert_eq!(cfg.ralph_args, vec!["--model", "opus", "--dir", "custom"]);
        assert_eq!(cfg.state_dir, PathBuf::from("/w/custom"));
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
        assert_eq!(cfg.state_dir, PathBuf::from("/repo/custom"));
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
        assert_eq!(cfg.state_dir, PathBuf::from("/repo/.ralph"));
        assert!(cfg.ralph_args.is_empty());
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
        assert!(!cfg.autostart); // default off

        let mut with_flag = base.to_vec();
        with_flag.push("--autostart");
        let argv: Vec<String> = with_flag.iter().map(|s| s.to_string()).collect();
        let cfg = parse(&argv, env_map(&[("DISCORD_BOT_TOKEN", "tok")])).unwrap();
        assert!(cfg.autostart); // flag turns it on

        let argv: Vec<String> = base.iter().map(|s| s.to_string()).collect();
        let cfg = parse(
            &argv,
            env_map(&[("DISCORD_BOT_TOKEN", "tok"), ("RALPHD_AUTOSTART", "1")]),
        )
        .unwrap();
        assert!(cfg.autostart); // env turns it on
        let cfg = parse(
            &argv,
            env_map(&[("DISCORD_BOT_TOKEN", "tok"), ("RALPHD_AUTOSTART", "false")]),
        )
        .unwrap();
        assert!(!cfg.autostart); // env 'false' stays off
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
        assert_eq!((cfg.guild_id, cfg.channel_id, cfg.user_id), (10, 20, 30));
    }
}
