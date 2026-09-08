use serde_json::Value;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Repo(PathBuf);

impl Repo {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "ralph-backlog-models-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join(".ralph")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        std::os::unix::fs::symlink("/usr/bin/git", root.join("bin/git")).unwrap();
        for agent in ["claude", "codex"] {
            let path = root.join("bin").join(agent);
            fs::write(&path, include_str!("fixtures/agent.py")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(root.join(".ralph/PROMPT.md"), "Work on the task.").unwrap();
        Self(root)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ralph"));
        for (key, _) in std::env::vars() {
            if key.starts_with("RALPH_") || key == "DISCORD_WEBHOOK" {
                cmd.env_remove(key);
            }
        }
        cmd.args(args)
            .current_dir(&self.0)
            .stdin(Stdio::null())
            .env("PATH", self.0.join("bin"))
            .env("TEST_AGENT_MODE", "complete");
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        out
    }

    fn backlog(&self) -> String {
        fs::read_to_string(self.0.join(".ralph/BACKLOG.md")).unwrap()
    }

    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(self.0.join("calls.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn pair(call: &Value, key: &str, value: &str) -> bool {
    call["args"]
        .as_array()
        .unwrap()
        .windows(2)
        .any(|args| args[0] == key && args[1] == value)
}

#[test]
fn add_models_survive_top_level_explicit_child_and_legacy_paths() {
    for flag in ["--model", "--tier", "-m"] {
        let repo = Repo::new();
        repo.ok(&["add", "Parent", flag, " Opus ", "--verify", "true"]);
        repo.ok(&["add", "1.1", "Child", flag, "!Opus", "--verify", "true"]);
        repo.ok(&[
            "add", "--under", "1", "Other", flag, "gpt-test", "--verify", "true",
        ]);
        repo.ok(&[
            "backlog", "add", "--title", "Legacy", flag, "!astra", "--verify", "true",
        ]);
        let text = repo.backlog();
        assert!(text.contains("**1 — Parent** @opus —\n"), "{text}");
        assert!(text.contains("  - [ ] **1.1 — Child** !opus —\n"), "{text}");
        assert!(
            text.contains("  - [ ] **1.2 — Other** @gpt-test —\n"),
            "{text}"
        );
        assert!(text.contains("**2 — Legacy** !astra —\n"), "{text}");
        repo.ok(&["lint"]);
    }
}

#[test]
fn piped_body_and_path_flags_keep_the_task_model() {
    let repo = Repo::new();
    let mut child = repo
        .command(&[
            "add",
            "Task",
            "--model",
            "!opus",
            "--dir",
            "custom",
            "--backlog",
            "tasks.md",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"Keep this constraint.\nVerify: true\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let text = fs::read_to_string(repo.0.join("tasks.md")).unwrap();
    assert!(
        text.contains("**1 — Task** !opus —\n  Keep this constraint.\n  Verify: true\n"),
        "{text}"
    );
    assert!(!repo.0.join(".ralph/BACKLOG.md").exists());
    repo.ok(&[
        "backlog",
        "edit",
        "--id",
        "1",
        "--title",
        "Renamed",
        "--verify",
        "true",
        "--dir",
        "custom",
        "--backlog",
        "tasks.md",
    ]);
    assert!(fs::read_to_string(repo.0.join("tasks.md"))
        .unwrap()
        .contains("**1 — Renamed** !opus —"));
}

#[test]
fn queued_model_survives_replay_and_edits_preserve_or_replace_it() {
    let repo = Repo::new();
    repo.ok(&["add", "Parent", "--verify", "true"]);
    let before = repo.backlog();
    fs::write(
        repo.0.join(".ralph/loop.pid"),
        std::process::id().to_string(),
    )
    .unwrap();
    let out = repo.ok(&[
        "add", "--under", "1", "Child", "--model", "!opus", "--verify", "true",
    ]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("queued"));
    assert_eq!(repo.backlog(), before);
    let request_path = fs::read_dir(repo.0.join(".ralph/inbox"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "json"))
        .unwrap();
    let request: Value = serde_json::from_str(&fs::read_to_string(request_path).unwrap()).unwrap();
    assert_eq!(request["model"], "!opus");
    fs::remove_file(repo.0.join(".ralph/loop.pid")).unwrap();
    repo.ok(&["add", "Drain", "--verify", "true"]);
    assert!(repo.backlog().contains("**1.1 — Child** !opus —"));
    repo.ok(&[
        "backlog", "edit", "--id", "1.1", "--title", "Renamed", "--verify", "true",
    ]);
    assert!(repo.backlog().contains("**1.1 — Renamed** !opus —"));
    repo.ok(&[
        "backlog", "edit", "--id", "1.1", "--title", "Renamed", "--verify", "true", "--model",
        "sonnet",
    ]);
    assert!(repo.backlog().contains("**1.1 — Renamed** @sonnet —"));
}

#[test]
fn invalid_models_and_unknown_flags_fail_without_mutation() {
    let repo = Repo::new();
    repo.ok(&["add", "Existing", "--verify", "true"]);
    let before = repo.backlog();
    for prefix in [
        vec!["add", "Bad"],
        vec!["backlog", "add", "--title", "Bad"],
        vec!["backlog", "edit", "--id", "1", "--title", "Bad"],
    ] {
        for options in [
            vec!["--model", "opuss"],
            vec!["--model", "!!opus"],
            vec!["--model", "!opus,sonnet"],
            vec!["--model", "opus\nInjected"],
            vec!["--modle", "opus"],
            vec!["--model"],
            vec!["--model", "opus", "--tier", "sonnet"],
        ] {
            let mut args = prefix.clone();
            args.extend(options);
            args.extend(["--verify", "true"]);
            let out = repo.run(&args);
            assert!(!out.status.success(), "accepted {args:?}");
            assert_eq!(repo.backlog(), before);
        }
    }
}

#[test]
fn inserted_soft_model_can_be_overridden_but_strict_model_cannot() {
    for (model, expected) in [("opus", "haiku"), ("!opus", "opus")] {
        let repo = Repo::new();
        repo.ok(&["add", "Task", "--model", model, "--verify", "true"]);
        repo.ok(&["model", "haiku"]);
        repo.ok(&["--once", "--model", "sonnet"]);
        let calls = repo.calls();
        assert_eq!(calls[0]["agent"], "claude");
        assert!(pair(&calls[0], "--model", expected), "{calls:?}");
        if model.starts_with('!') {
            assert!(!calls[0]["args"]
                .as_array()
                .unwrap()
                .contains(&Value::from("--fallback-model")));
        }
    }
}

#[test]
fn inserted_strict_model_retries_same_provider_while_soft_model_fails_over() {
    for (model, expected_agent, expected_model) in [
        ("opus", "codex", "gpt-5.6-sol"),
        ("!opus", "claude", "opus"),
    ] {
        let repo = Repo::new();
        fs::write(repo.0.join(".ralph/ralph.toml"), "limit_wait = 0\n").unwrap();
        repo.ok(&["add", "Task", "--model", model, "--verify", "true"]);
        let out = repo
            .command(&["--once"])
            .env("TEST_AGENT_MODE", "claude-depleted")
            .env("TEST_DEPLETED_CALLS", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let calls = repo.calls();
        assert_eq!(calls[0]["agent"], "claude");
        assert_eq!(calls[1]["agent"], expected_agent, "{calls:?}");
        assert!(pair(&calls[1], "--model", expected_model), "{calls:?}");
    }
}
