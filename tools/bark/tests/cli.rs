//! End-to-end CLI behavior. Nothing here touches the network: every send uses
//! `--dry-run`, so the tests exercise config resolution and rendering only.

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

fn scratch() -> std::path::PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "bark-cli-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

/// Run bark with a scratch config path and no inherited webhook env.
fn bark(config: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bark"))
        .args(args)
        .env("BARK_CONFIG", config)
        .env_remove("BARK_WEBHOOK")
        .output()
        .unwrap()
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn help_documents_the_message_shape() {
    let out = Command::new(env!("CARGO_BIN_EXE_bark"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(out.status.success());
    let help = stdout(&out);
    assert!(help.contains("<id> <message...>"));
    assert!(help.contains("--to <name>"));
    assert!(help.contains("--dry-run"));
    assert!(stderr(&out).is_empty());
}

#[test]
fn init_then_send_dry_run() {
    let root = scratch();
    let config = root.join("config.toml");

    let out = bark(
        &config,
        &["init", "--webhook", "https://example/hooks/1/tokenvalue"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains(config.to_str().unwrap()));

    // The config holds a token: owner-only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&config).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config should be 0600, got {mode:o}");
    }

    // Re-running refuses to clobber, unless forced.
    let out = bark(
        &config,
        &["init", "--webhook", "https://example/hooks/2/other"],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("already exists"), "{}", stderr(&out));
    let out = bark(
        &config,
        &[
            "init",
            "--force",
            "--webhook",
            "https://example/hooks/2/other",
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));

    let out = bark(&config, &["--dry-run", "build-42", "all", "green"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let shown = stdout(&out);
    assert!(shown.contains("content: `[build-42]` all green"), "{shown}");
    // Never print the token, even locally.
    assert!(shown.contains("othe***"), "{shown}");
    assert!(!shown.contains("other\n"), "{shown}");
}

#[test]
fn init_without_a_url_says_so() {
    let root = scratch();
    let out = bark(&root.join("config.toml"), &["init"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("--webhook"), "{}", stderr(&out));
}

#[test]
fn named_targets_are_selectable_and_listable() {
    let root = scratch();
    let config = root.join("config.toml");
    fs::write(
        &config,
        "default = \"ops\"\n\
         [targets.ops]\nwebhook = \"https://example/hooks/1/opstoken\"\n\
         [targets.alerts]\nwebhook = \"https://example/hooks/2/alerttoken\"\n",
    )
    .unwrap();

    let out = bark(&config, &["targets"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let listing = stdout(&out);
    assert!(listing.contains("* ops"), "{listing}");
    assert!(listing.contains("  alerts"), "{listing}");
    assert!(!listing.contains("opstoken"), "{listing}");

    let out = bark(
        &config,
        &["--dry-run", "--to", "alerts", "oncall", "disk full"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("target:  alerts"), "{}", stdout(&out));

    let out = bark(&config, &["--dry-run", "--to", "nope", "id", "hi"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("known: alerts, ops"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn missing_config_names_the_path_it_looked_at_and_the_fix() {
    let root = scratch();
    let missing = root.join("nope.toml");
    let out = bark(&missing, &["--dry-run", "id", "hi"]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.contains(missing.to_str().unwrap()), "{err}");
    assert!(err.contains("file not found"), "{err}");
    assert!(err.contains("bark init --webhook"), "{err}");
    assert!(err.contains("$BARK_WEBHOOK"), "{err}");
}

#[test]
fn targets_on_an_empty_config_still_says_where_it_looked() {
    let root = scratch();
    let missing = root.join("nope.toml");
    let out = bark(&missing, &["targets"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let shown = stdout(&out);
    assert!(shown.contains(missing.to_str().unwrap()), "{shown}");
    assert!(shown.contains("no targets"), "{shown}");
}

#[test]
fn malformed_config_names_the_file() {
    let root = scratch();
    let config = root.join("config.toml");
    fs::write(&config, "webhook = [").unwrap();
    let out = bark(&config, &["--dry-run", "id", "hi"]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.contains("config.toml"), "{err}");
}

#[test]
fn env_webhook_is_a_fallback_and_flags_win() {
    let root = scratch();
    let config = root.join("config.toml");
    fs::write(&config, "webhook = \"https://example/hooks/1/cfgtoken\"").unwrap();

    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_bark"))
            .args(args)
            .env("BARK_CONFIG", &config)
            .env("BARK_WEBHOOK", "https://example/hooks/9/envtoken")
            .output()
            .unwrap()
    };

    // Env beats the config file...
    let out = run(&["--dry-run", "id", "hi"]);
    let shown = stdout(&out);
    assert!(shown.contains("$BARK_WEBHOOK"), "{shown}");
    assert!(shown.contains("envt***"), "{shown}");
    // ...an explicit --to goes back to the config...
    let out = run(&["--dry-run", "--to", "default", "id", "hi"]);
    assert!(stdout(&out).contains("cfgt***"), "{}", stdout(&out));
    // ...and --webhook beats everything.
    let out = run(&[
        "--dry-run",
        "--webhook",
        "https://example/hooks/3/flag",
        "id",
        "hi",
    ]);
    assert!(
        stdout(&out).contains("target:  --webhook"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn stdin_becomes_the_message() {
    let root = scratch();
    let config = root.join("config.toml");
    fs::write(&config, "webhook = \"https://example/hooks/1/tok\"").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_bark"))
        .args(["--dry-run", "log-7", "-"])
        .env("BARK_CONFIG", &config)
        .env_remove("BARK_WEBHOOK")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"line one\nline two\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("content: `[log-7]` line one\nline two"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn empty_stdin_is_refused() {
    let root = scratch();
    let config = root.join("config.toml");
    fs::write(&config, "webhook = \"https://example/hooks/1/tok\"").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_bark"))
        .args(["--dry-run", "log-7", "-"])
        .env("BARK_CONFIG", &config)
        .env_remove("BARK_WEBHOOK")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("stdin was empty"), "{}", stderr(&out));
}

#[test]
fn username_override_is_reported() {
    let root = scratch();
    let config = root.join("config.toml");
    fs::write(
        &config,
        "webhook = \"https://example/hooks/1/tok\"\nusername = \"suitcase\"\n",
    )
    .unwrap();

    let out = bark(&config, &["--dry-run", "id", "hi"]);
    assert!(
        stdout(&out).contains("as:      suitcase"),
        "{}",
        stdout(&out)
    );
    let out = bark(&config, &["--dry-run", "--username", "ci", "id", "hi"]);
    assert!(stdout(&out).contains("as:      ci"), "{}", stdout(&out));
}
