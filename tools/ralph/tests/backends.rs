use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Repo(PathBuf);
impl Repo {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "ralph-backends-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join(".ralph")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        for agent in ["claude", "codex"] {
            let path = root.join("bin").join(agent);
            fs::write(&path, include_str!("fixtures/agent.py")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(root.join(".ralph/BACKLOG.md"), "<!-- ralph-backlog: v2 -->\n# Backlog\n- [x] **1 — Done.**\n  Verify: `true` exits 0.\n").unwrap();
        fs::write(root.join(".ralph/PROMPT.md"), "Work on the selected task.").unwrap();
        fs::write(
            root.join(".gitignore"),
            "/.ralph/\n/bin/\n/calls.jsonl\n/child.pid\n",
        )
        .unwrap();
        let repo = Self(root);
        repo.git(&["init", "-q"]);
        repo.git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "Initial",
        ]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.invalid"]);
        repo
    }

    fn git(&self, args: &[&str]) {
        assert!(Command::new("git")
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap()
            .status
            .success());
    }

    fn config(&self, text: &str) {
        fs::write(self.0.join(".ralph/ralph.toml"), text).unwrap();
    }
    fn backlog(&self) {
        fs::write(self.0.join(".ralph/BACKLOG.md"), "<!-- ralph-backlog: v2 -->\n# Backlog\n\n- [ ] **1 — Implement task.** @opus — a substantial task.\n  Verify: `true` exits 0.\n").unwrap();
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ralph"));
        for (key, _) in std::env::vars() {
            if key.starts_with("RALPH_") || key == "DISCORD_WEBHOOK" {
                cmd.env_remove(key);
            }
        }
        cmd.args(args).current_dir(&self.0).env(
            "PATH",
            format!("{}:/usr/bin:/bin", self.0.join("bin").display()),
        );
        cmd
    }
    fn run(&self, args: &[&str], mode: &str) -> Output {
        self.command(args)
            .env("TEST_AGENT_MODE", mode)
            .output()
            .unwrap()
    }
    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(self.0.join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn success(out: &Output) {
    assert!(out.status.success(), "{out:?}");
}
fn has_pair(call: &Value, key: &str, value: &str) -> bool {
    call["args"]
        .as_array()
        .unwrap()
        .windows(2)
        .any(|w| w[0] == key && w[1] == value)
}

#[test]
fn opus_launch_and_legacy_result_are_unchanged() {
    let repo = Repo::new();
    success(&repo.run(&["--model", "opus", "--once"], "complete"));
    let calls = repo.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["agent"], "claude");
    assert!(has_pair(&calls[0], "--model", "opus"));
    let result: Value =
        serde_json::from_str(&fs::read_to_string(repo.0.join(".ralph/last-result.json")).unwrap())
            .unwrap();
    assert_eq!(result["total_cost_usd"], 0.25);
}

#[test]
fn openai_model_selects_codex_and_normalizes_result_without_schema_changes() {
    let repo = Repo::new();
    success(&repo.run(&["-m", "gpt-test", "--once"], "complete"));
    let calls = repo.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["agent"], "codex");
    assert!(has_pair(&calls[0], "--model", "gpt-test"));
    let args = calls[0]["args"].as_array().unwrap();
    assert!(args.contains(&Value::from("--ephemeral")));
    assert!(!args.contains(&Value::from("--fallback-model")));
    assert!(calls[0]["prompt"]
        .as_str()
        .unwrap()
        .starts_with("Work on the selected task."));
    let result: Value =
        serde_json::from_str(&fs::read_to_string(repo.0.join(".ralph/last-result.json")).unwrap())
            .unwrap();
    assert_eq!(result["type"], "result");
    assert_eq!(result["result"], "RALPH_COMPLETE");
    assert_eq!(result["usage"]["input_tokens"], 40);
    assert_eq!(result["usage"]["cache_read_input_tokens"], 60);
    assert_eq!(result["usage"]["output_tokens"], 12);
    assert_eq!(
        fs::read_to_string(repo.0.join(".ralph/iteration")).unwrap(),
        "0\n"
    );
}

#[test]
fn tiers_route_to_codex_models_and_helpers_follow_the_backend() {
    let repo = Repo::new();
    repo.backlog();
    repo.config("backend = 'openai'\n[tier_models]\nopus = 'gpt-worker'\nsonnet = 'gpt-helper'\n");
    success(&repo.run(&["--once", "--no-yolo"], "review"));
    let calls = repo.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(has_pair(&calls[0], "--model", "gpt-worker"));
    assert!(has_pair(&calls[0], "-c", "model_reasoning_effort=\"high\""));
    assert!(has_pair(&calls[0], "--sandbox", "workspace-write"));
    assert!(has_pair(&calls[1], "--model", "gpt-helper"));
    assert!(has_pair(&calls[1], "--sandbox", "read-only"));
    assert_eq!(
        fs::read_to_string(repo.0.join(".ralph/PROGRESS.md")).unwrap(),
        "- carry this constraint forward"
    );
    assert!(!fs::read_to_string(repo.0.join(".ralph/run.log"))
        .unwrap()
        .contains("→ COMPLETE"));
}

#[test]
fn one_shot_accepts_opus_outside_ladder_and_concrete_models_in_custom_dir() {
    let repo = Repo::new();
    repo.config("escalation_ladder = ['sonnet']\n");
    success(&repo.run(&["model", " Opus "], "complete"));
    success(&repo.run(&["--once"], "complete"));
    assert!(has_pair(&repo.calls()[0], "--model", "opus"));
    success(&repo.run(&["model", "gpt-test", "--dir", "custom"], "complete"));
    assert_eq!(
        fs::read_to_string(repo.0.join("custom/MODEL")).unwrap(),
        "gpt-test\n"
    );
    fs::write(
        repo.0.join(".ralph/BACKLOG.md"),
        "<!-- ralph-backlog: v2 -->\n# Backlog\n- [x] **1 — Done.**\n  Verify: `true` exits 0.\n",
    )
    .unwrap();
    fs::remove_file(repo.0.join("bin/claude")).unwrap();
    success(&repo.run(&["--once", "--dir", "custom"], "complete"));
    assert_eq!(repo.calls()[1]["agent"], "codex");
    assert!(!repo.0.join("custom/MODEL").exists());
}

#[test]
fn codex_message_resumes_and_exposes_compatible_stream() {
    let repo = Repo::new();
    success(&repo.run(
        &[
            "msg",
            "--backend",
            "codex",
            "--model",
            "custom-model",
            "hello",
        ],
        "review",
    ));
    let output = repo.run(&["msg", "--stream-json", "follow up"], "review");
    success(&output);
    let calls = repo.calls();
    assert_eq!(calls.len(), 2);
    assert!(has_pair(
        &calls[1],
        "resume",
        "11111111-2222-3333-4444-555555555555"
    ));
    assert!(has_pair(&calls[1], "--model", "custom-model"));
    assert!(!calls[0]["args"]
        .as_array()
        .unwrap()
        .contains(&Value::from("--ephemeral")));
    let events: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert!(events.iter().any(|v| v["type"] == "assistant"));
    assert_eq!(events.last().unwrap()["type"], "result");
    let output = repo.run(&["msg", "--model", "bad-model", "hello"], "fatal");
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(repo.0.join(".ralph/msg-model")).unwrap(),
        "custom-model\n"
    );
    success(&repo.run(&["msg", "--new"], "review"));
    assert!(!repo.0.join(".ralph/msg-session").exists());
    assert!(!repo.0.join(".ralph/msg-backend").exists());
}

#[test]
fn codex_failures_abort_and_unknown_cost_budgets_are_rejected() {
    for mode in ["fatal", "stderr-error"] {
        let repo = Repo::new();
        let out = repo.run(&["--backend", "codex", "--once"], mode);
        assert!(!out.status.success(), "{out:?}");
        assert_eq!(repo.calls().len(), 1);
        assert!(fs::read_to_string(repo.0.join(".ralph/run.log"))
            .unwrap()
            .contains("ABORTED (fatal)"));
    }
    let repo = Repo::new();
    let out = repo.run(&["--backend", "codex", "--max-cost", "1"], "complete");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("does not report USD cost"));
    assert!(repo.calls().is_empty());
}

#[test]
fn codex_judge_can_refute_a_committed_checkoff() {
    let repo = Repo::new();
    repo.backlog();
    repo.config("backend = 'codex'\njudge_tiers = ['opus']\njudge_model = 'gpt-judge'\n");
    success(&repo.run(&["--once"], "commit"));
    let calls = repo.calls();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert!(has_pair(&calls[1], "--model", "gpt-judge"));
    assert!(fs::read_to_string(repo.0.join(".ralph/BACKLOG.md"))
        .unwrap()
        .contains("- [ ]"));
    assert!(fs::read_to_string(repo.0.join(".ralph/run.log"))
        .unwrap()
        .contains("REFUTE"));
}

#[test]
fn codex_timeout_kills_the_process_tree_and_aborts_at_threshold() {
    let repo = Repo::new();
    repo.config("backend = 'codex'\niteration_timeout = 1\nescalate_after = 1\nabort_after = 1\ntransient_wait = 0\n");
    let started = std::time::Instant::now();
    let out = repo.run(&[], "hang");
    assert!(!out.status.success());
    assert!(started.elapsed().as_secs() < 10);
    let pid = fs::read_to_string(repo.0.join("child.pid")).unwrap();
    // A killed child can briefly be a zombie waiting for init to reap it.
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    assert!(
        stat.is_empty() || stat.split_whitespace().nth(2) == Some("Z"),
        "child still running: {stat}"
    );
}

#[test]
fn codex_learning_miner_uses_configured_model() {
    let repo = Repo::new();
    repo.config("backend = 'codex'\nsynth_model = 'custom-miner'\n");
    fs::write(
        repo.0.join(".ralph/run.log"),
        "a recurring failure worth inspecting",
    )
    .unwrap();
    success(&repo.run(&["learn"], "review"));
    let calls = repo.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["agent"], "codex");
    assert!(has_pair(&calls[0], "--model", "custom-miner"));
}

#[test]
fn codex_rate_limits_and_transient_errors_use_existing_retry_policy() {
    for mode in ["limit-retry", "transient-retry"] {
        let repo = Repo::new();
        repo.config("backend = 'codex'\nlimit_wait = 0\ntransient_wait = 0\n");
        success(&repo.run(&["--once"], mode));
        assert_eq!(repo.calls().len(), 2);
        let ledger = fs::read_to_string(repo.0.join(".ralph/ledger.jsonl")).unwrap();
        assert_eq!(ledger.lines().count(), 2);
    }
}

#[test]
fn changing_message_backend_archives_only_after_success() {
    let repo = Repo::new();
    success(&repo.run(&["msg", "--model", "opus", "hello"], "review"));
    let old = fs::read_to_string(repo.0.join(".ralph/msg-session")).unwrap();
    assert!(!repo
        .run(
            &["msg", "--backend", "codex", "--model", "gpt-test", "hello"],
            "fatal"
        )
        .status
        .success());
    assert_eq!(
        fs::read_to_string(repo.0.join(".ralph/msg-session")).unwrap(),
        old
    );
    success(&repo.run(
        &["msg", "--backend", "codex", "--model", "gpt-test", "hello"],
        "review",
    ));
    assert_eq!(
        fs::read_to_string(repo.0.join(".ralph/msg-backend")).unwrap(),
        "codex\n"
    );
    assert!(fs::read_dir(repo.0.join(".ralph/archive"))
        .unwrap()
        .any(|entry| { fs::read_to_string(entry.unwrap().path()).unwrap() == old }));
}

#[test]
fn concrete_openai_model_escalates_effort_without_switching_to_claude() {
    let repo = Repo::new();
    success(&repo.run(&["--model", "gpt-test", "--max-iterations", "3"], "stall"));
    let calls = repo.calls();
    let workers: Vec<_> = calls
        .iter()
        .filter(|c| {
            !c["prompt"]
                .as_str()
                .unwrap()
                .starts_with("You are a note-taker")
        })
        .collect();
    assert_eq!(workers.len(), 3);
    for worker in &workers {
        assert_eq!(worker["agent"], "codex");
        assert!(has_pair(worker, "--model", "gpt-test"));
    }
    assert!(has_pair(
        workers[0],
        "-c",
        "model_reasoning_effort=\"medium\""
    ));
    assert!(has_pair(
        workers[2],
        "-c",
        "model_reasoning_effort=\"high\""
    ));
}

#[test]
fn custom_model_repins_keep_the_codex_conversation() {
    let repo = Repo::new();
    success(&repo.run(
        &[
            "msg",
            "--backend",
            "codex",
            "--model",
            "custom-one",
            "hello",
        ],
        "review",
    ));
    success(&repo.run(&["msg", "--model", "custom-two", "follow up"], "review"));
    let calls = repo.calls();
    assert_eq!(calls[1]["agent"], "codex");
    assert!(has_pair(&calls[1], "--model", "custom-two"));
    assert!(has_pair(
        &calls[1],
        "resume",
        "11111111-2222-3333-4444-555555555555"
    ));
    assert_eq!(
        fs::read_to_string(repo.0.join(".ralph/msg-model")).unwrap(),
        "custom-two\n"
    );
    assert!(
        !repo.0.join(".ralph/archive").exists()
            || fs::read_dir(repo.0.join(".ralph/archive"))
                .unwrap()
                .next()
                .is_none()
    );
}

#[test]
fn claude_model_suffixes_and_fallback_lists_reach_the_cli() {
    let repo = Repo::new();
    success(&repo.run(
        &[
            "--model",
            "opus[1m]",
            "--fallback-model",
            "sonnet,haiku",
            "--once",
        ],
        "complete",
    ));
    let calls = repo.calls();
    assert_eq!(calls[0]["agent"], "claude");
    assert!(has_pair(&calls[0], "--model", "opus[1m]"));
    assert!(has_pair(&calls[0], "--fallback-model", "sonnet,haiku"));
}

#[test]
fn duplicate_model_mappings_escalate_from_medium_to_high() {
    let repo = Repo::new();
    repo.config("model = 'gpt-test'\n[tier_models]\nhaiku = 'gpt-test'\nsonnet = 'gpt-test'\nopus = 'gpt-test'\n");
    success(&repo.run(&["--max-iterations", "3"], "stall"));
    let calls = repo.calls();
    let workers: Vec<_> = calls
        .iter()
        .filter(|c| {
            !c["prompt"]
                .as_str()
                .unwrap()
                .starts_with("You are a note-taker")
        })
        .collect();
    assert_eq!(workers.len(), 3);
    assert!(has_pair(
        workers[0],
        "-c",
        "model_reasoning_effort=\"medium\""
    ));
    assert!(has_pair(
        workers[2],
        "-c",
        "model_reasoning_effort=\"high\""
    ));
}

#[test]
fn model_help_and_unsupported_flags_do_not_write_an_override() {
    let repo = Repo::new();
    success(&repo.run(&["model", "opus", "--help"], "complete"));
    assert!(!repo.0.join(".ralph/MODEL").exists());
    assert!(!repo
        .run(&["model", "opus", "--backend", "codex"], "complete")
        .status
        .success());
    assert!(!repo.0.join(".ralph/MODEL").exists());
}

#[test]
fn message_backend_override_survives_conflicting_loop_config() {
    let repo = Repo::new();
    repo.config("backend = 'codex'\n");
    success(&repo.run(
        &["msg", "--backend", "claude", "--model", "opus", "hello"],
        "review",
    ));
    success(&repo.run(&["msg", "follow up"], "review"));
    let calls = repo.calls();
    assert_eq!(calls[1]["agent"], "claude");
    let session = fs::read_to_string(repo.0.join(".ralph/msg-session")).unwrap();
    assert!(has_pair(&calls[1], "--resume", session.trim()));
    assert!(has_pair(&calls[1], "--model", "opus"));
}
