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
        // Keep tests offline even when real provider CLIs are installed globally.
        std::os::unix::fs::symlink("/usr/bin/git", root.join("bin/git")).unwrap();
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
        cmd.args(args)
            .current_dir(&self.0)
            .env("TEST_RALPH_BIN", env!("CARGO_BIN_EXE_ralph"))
            .env("PATH", self.0.join("bin"));
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

#[test]
fn final_marker_cannot_bypass_git_audit_or_queued_new_work() {
    let repo = Repo::new();
    let out = repo.run(&["--once"], "switch-complete");
    assert_eq!(out.status.code(), Some(1));
    assert!(repo.0.join(".ralph/BACKLOG.md").exists());
    let state: Value =
        serde_json::from_str(&fs::read_to_string(repo.0.join(".ralph/run.json")).unwrap()).unwrap();
    assert_eq!(state["terminal_reason"], "Git contract breach");

    let repo = Repo::new();
    success(&repo.run(&["--once"], "queue-add-complete"));
    assert!(fs::read_to_string(repo.0.join(".ralph/BACKLOG.md"))
        .unwrap()
        .contains("Follow-up"));
    assert!(!fs::read_to_string(repo.0.join(".ralph/run.json"))
        .unwrap()
        .contains("\"terminal_reason\": \"complete\""));
}

#[test]
fn opt_in_gate_blocks_direct_and_queued_closure_and_preserves_feedback() {
    for mode in ["commit-complete", "queue-complete"] {
        let repo = Repo::new();
        repo.backlog();
        repo.config(
            "[acceptance.'1']\ncommand = ['/bin/sh', '-c', 'echo missing-case >&2; exit 7']\n",
        );
        success(&repo.run(&["--once"], mode));
        assert!(fs::read_to_string(repo.0.join(".ralph/BACKLOG.md"))
            .unwrap()
            .contains("- [ ] **1"));
        let feedback = fs::read_to_string(repo.0.join(".ralph/previous-attempt.json")).unwrap();
        assert!(feedback.contains("missing-case"));
        let next = repo.run(&["brief", "--full"], "review");
        success(&next);
        assert!(String::from_utf8_lossy(&next.stdout).contains("missing-case"));
        success(&repo.run(&["--once"], "review"));
        assert!(fs::read_to_string(repo.0.join(".ralph/BACKLOG.md"))
            .unwrap()
            .contains("- [ ] **1"));
    }
}

#[test]
fn prose_only_task_completes_without_extra_calls_and_status_survives_archive() {
    let repo = Repo::new();
    repo.backlog();
    success(&repo.run(&["--once"], "queue-complete"));
    assert_eq!(repo.calls().len(), 1);
    assert!(!repo.0.join(".ralph/BACKLOG.md").exists());
    let status = repo.run(&["status", "--json"], "complete");
    success(&status);
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["run"]["terminal_reason"], "complete");
    assert_eq!(status["running"], false);
    let attempt = PathBuf::from(status["run"]["artifacts"].as_str().unwrap());
    assert!(repo.0.join(&attempt).join("accepted.json").exists());
    assert_eq!(
        fs::read_to_string(repo.0.join(attempt).join("prompt.md")).unwrap(),
        repo.calls()[0]["prompt"].as_str().unwrap()
    );
}

#[test]
fn required_and_advisory_reviews_distinguish_unavailable() {
    for (policy, response, complete) in [
        ("required", "garbled", false),
        ("required", "PASS", true),
        ("advisory", "REFUTE: a subjective concern", true),
        ("advisory", "garbled", true),
    ] {
        let repo = Repo::new();
        repo.backlog();
        repo.config(&format!("[acceptance.'1']\nreview = '{policy}'\n"));
        let out = repo
            .command(&["--once"])
            .env("TEST_AGENT_MODE", "queue-complete")
            .env("TEST_REVIEW_RESULT", response)
            .output()
            .unwrap();
        assert_eq!(out.status.success(), complete, "{out:?}");
        assert_eq!(repo.0.join(".ralph/BACKLOG.md").exists(), !complete);
        assert_eq!(repo.calls().len(), 2);
    }
}

#[test]
fn full_preview_matches_launch_and_doctor_is_read_only() {
    let repo = Repo::new();
    repo.backlog();
    fs::write(repo.0.join(".ralph/PROGRESS.md"), "é".repeat(4000)).unwrap();
    let preview = repo.run(&["brief", "--full"], "review");
    success(&preview);
    assert!(String::from_utf8_lossy(&preview.stdout).contains("Carry-forward truncated"));
    let doctor = repo.run(&["doctor", "--json"], "review");
    success(&doctor);
    assert_eq!(
        serde_json::from_slice::<Value>(&doctor.stdout).unwrap()["passed"],
        true
    );
    assert!(repo.calls().is_empty());
    assert!(!repo.0.join(".ralph/run.json").exists());
    success(&repo.run(&["--once"], "review"));
    assert_eq!(
        String::from_utf8(preview.stdout).unwrap(),
        repo.calls()[0]["prompt"].as_str().unwrap()
    );

    fs::write(repo.0.join(".ralph/PROMPT.md"), "Goal: {{DEFINE ME}}").unwrap();
    assert_eq!(
        repo.run(&["doctor", "--json"], "review").status.code(),
        Some(1)
    );
}

#[test]
fn non_code_stalls_survive_restart_and_contract_edits_reset_them() {
    let repo = Repo::new();
    repo.backlog();
    for _ in 0..2 {
        success(&repo.run(&["--once"], "review"));
    }
    let first: Value =
        serde_json::from_str(&fs::read_to_string(repo.0.join(".ralph/thrash.json")).unwrap())
            .unwrap();
    assert_eq!(first["attempts"], 2);
    assert_eq!(first["streak"], 1);
    success(&repo.run(&["--once"], "review"));
    let state: Value =
        serde_json::from_str(&fs::read_to_string(repo.0.join(".ralph/thrash.json")).unwrap())
            .unwrap();
    assert_eq!(state["streak"], 2);
    let backlog = repo.0.join(".ralph/BACKLOG.md");
    let text = fs::read_to_string(&backlog)
        .unwrap()
        .replace("a substantial task", "reframed task");
    fs::write(backlog, text).unwrap();
    success(&repo.run(&["--once"], "review"));
    let state: Value =
        serde_json::from_str(&fs::read_to_string(repo.0.join(".ralph/thrash.json")).unwrap())
            .unwrap();
    assert_eq!(state["attempts"], 1);
    assert_eq!(state["streak"], 0);
}

#[test]
fn verification_timeout_has_a_receipt_and_keeps_the_task_open() {
    let repo = Repo::new();
    repo.backlog();
    repo.config("[acceptance.'1']\ncommand = ['/bin/sleep', '30']\ntimeout_secs = 1\n");
    success(&repo.run(&["--once"], "queue-complete"));
    let state: Value =
        serde_json::from_str(&fs::read_to_string(repo.0.join(".ralph/run.json")).unwrap()).unwrap();
    let evidence = repo.0.join(state["artifacts"].as_str().unwrap());
    let receipt: Value =
        serde_json::from_str(&fs::read_to_string(evidence.join("verification.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["timed_out"], true);
    assert!(fs::read_to_string(repo.0.join(".ralph/BACKLOG.md"))
        .unwrap()
        .contains("- [ ] **1"));
}

#[test]
fn forced_stop_reaches_helpers_and_restart_revokes_unaccepted_closure() {
    let repo = Repo::new();
    repo.backlog();
    let mut worker = repo
        .command(&["--once"])
        .env("TEST_AGENT_MODE", "review")
        .env("TEST_PAUSE_HELPER", "1")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_for(|| repo.calls().len() >= 2);
    let mut stopper = repo
        .command(&["stop", "--force"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_for(|| stopper.try_wait().unwrap().is_some());
    wait_for(|| worker.try_wait().unwrap().is_some());
    assert!(!repo.0.join(".ralph/STOP").exists());

    let repo = Repo::new();
    repo.backlog();
    let mut worker = repo
        .command(&["--once"])
        .env("TEST_AGENT_MODE", "queue-complete")
        .env("TEST_PAUSE_AFTER_CLOSURE", "1")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_for(|| repo.0.join("closure.ready").exists());
    let mut stopper = repo
        .command(&["stop", "--force"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_for(|| stopper.try_wait().unwrap().is_some());
    wait_for(|| worker.try_wait().unwrap().is_some());
    success(&repo.run(&["--once"], "review"));
    assert!(fs::read_to_string(repo.0.join(".ralph/BACKLOG.md"))
        .unwrap()
        .contains("- [ ] **1"));
    let calls = repo.calls();
    assert!(calls[1]["prompt"]
        .as_str()
        .unwrap()
        .contains("interrupted before acceptance"));
}

#[test]
fn final_policy_waits_for_a_separate_completion_audit() {
    let repo = Repo::new();
    repo.backlog();
    repo.config("[acceptance.'@complete']\ncommand = ['/bin/true']\n");
    success(&repo.run(&["--once"], "queue-complete"));
    assert!(repo.0.join(".ralph/BACKLOG.md").exists());
    success(&repo.run(&["--once"], "complete"));
    assert!(!repo.0.join(".ralph/BACKLOG.md").exists());
    let state: Value =
        serde_json::from_str(&fs::read_to_string(repo.0.join(".ralph/run.json")).unwrap()).unwrap();
    let evidence = repo.0.join(state["artifacts"].as_str().unwrap());
    let receipt: Value =
        serde_json::from_str(&fs::read_to_string(evidence.join("verification.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["exit_code"], 0);
}

#[test]
fn judge_uses_the_frozen_contract_even_if_worker_edits_it() {
    let repo = Repo::new();
    repo.backlog();
    repo.config("[acceptance.'1']\nreview = 'required'\n");
    let out = repo
        .command(&["--once"])
        .env("TEST_AGENT_MODE", "queue-complete")
        .env("TEST_WEAKEN_CONTRACT", "1")
        .env("TEST_REVIEW_RESULT", "PASS")
        .output()
        .unwrap();
    success(&out);
    let calls = repo.calls();
    assert!(calls[1]["prompt"]
        .as_str()
        .unwrap()
        .contains("Verify: `true` exits 0."));
    assert!(!calls[1]["prompt"]
        .as_str()
        .unwrap()
        .contains("Verify: trust the summary."));
}

#[test]
fn configured_attempt_cap_counts_productive_commits_on_an_unfinished_task() {
    let repo = Repo::new();
    repo.backlog();
    repo.config("task_attempt_limit = 1\n");
    success(&repo.run(&["--once"], "incremental-commit"));
    let calls_before = repo.calls().len();
    let out = repo.run(&["--once"], "review");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(repo.calls().len(), calls_before);
    let status = repo.run(&["status", "--json"], "review");
    success(&status);
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["run"]["task"], "1");
    assert!(status["run"]["terminal_reason"]
        .as_str()
        .unwrap()
        .contains("task attempt limit"));
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

fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !ready() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for fixture"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn stop_waits_for_the_iteration_but_async_returns_immediately() {
    let repo = Repo::new();
    repo.backlog();
    let mut worker = repo
        .command(&[])
        .env("TEST_PAUSE_WORKER", "1")
        .env("TEST_AGENT_MODE", "review")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_for(|| !repo.calls().is_empty());
    success(&repo.run(&["stop", "--async"], "review"));
    assert!(worker.try_wait().unwrap().is_none());
    let mut stopper = repo
        .command(&["stop"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(stopper.try_wait().unwrap().is_none());
    fs::write(repo.0.join("release.worker"), "").unwrap();
    wait_for(|| stopper.try_wait().unwrap().is_some());
    assert!(worker.wait().unwrap().success());
}

#[test]
fn forced_stop_tears_down_worker_even_under_supervision() {
    for flag in ["--force", "--now"] {
        let repo = Repo::new();
        let mut worker = repo
            .command(&["--restart", "true"])
            .env("TEST_AGENT_MODE", "hang")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        wait_for(|| repo.0.join("child.pid").exists());
        let mut stopper = repo
            .command(&["stop", flag])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        wait_for(|| stopper.try_wait().unwrap().is_some());
        wait_for(|| worker.try_wait().unwrap().is_some());
        let pid = fs::read_to_string(repo.0.join("child.pid")).unwrap();
        wait_for(|| {
            let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            stat.is_empty()
                || stat
                    .rsplit_once(") ")
                    .is_some_and(|(_, tail)| tail.starts_with('Z'))
        });
        assert_eq!(repo.calls().len(), 1, "a forced stop must suppress restart");
    }
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
        assert!(repo.calls().iter().all(|c| c["agent"] == "codex"));
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

#[test]
fn depletion_fails_over_by_default_in_both_directions() {
    for (model, mode, target, cli) in [
        ("claude-fable-5", "claude-depleted", "gpt-6-astra", "codex"),
        ("opus", "claude-depleted", "gpt-5.6-sol", "codex"),
        (
            "gpt-6-astra",
            "codex-depleted",
            "claude-fable-5-1",
            "claude",
        ),
        ("gpt-5.6-sol", "codex-depleted", "opus", "claude"),
        ("sonnet", "claude-stderr-depleted", "gpt-5.6-terra", "codex"),
    ] {
        let repo = Repo::new();
        repo.config("limit_wait = 0\n");
        success(&repo.run(&["--model", model, "--once"], mode));
        let calls = repo.calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert_eq!(calls[1]["agent"], cli);
        assert!(has_pair(&calls[1], "--model", target), "{calls:?}");
        assert_eq!(calls[0]["prompt"], calls[1]["prompt"]);
        let ledger = fs::read_to_string(repo.0.join(".ralph/ledger.jsonl")).unwrap();
        assert_eq!(ledger.lines().count(), 2);
        assert!(ledger.lines().last().unwrap().contains(target));
        let logs: String = fs::read_dir(repo.0.join(".ralph/logs"))
            .unwrap()
            .map(|p| fs::read_to_string(p.unwrap().path()).unwrap())
            .collect();
        assert!(logs.contains("RALPH_COMPLETE"));
        assert!(logs.contains("limit") || logs.contains("quota"));
    }
}

#[test]
fn claude_session_limit_fails_over_without_waiting_for_reset() {
    let repo = Repo::new();
    let out = repo
        .command(&[
            "--model",
            "claude-fable-5-1",
            "--once",
            "--max-duration",
            "2s",
        ])
        .env("TEST_AGENT_MODE", "claude-depleted")
        .env(
            "TEST_CLAUDE_LIMIT_MESSAGE",
            "You've hit your session limit · resets 10:20pm (America/Chicago)",
        )
        .output()
        .unwrap();
    success(&out);
    let calls = repo.calls();
    assert_eq!(
        calls.len(),
        2,
        "session exhaustion should immediately retry on Codex"
    );
    assert_eq!(calls[0]["agent"], "claude");
    assert_eq!(calls[1]["agent"], "codex");
    assert!(has_pair(&calls[1], "--model", "gpt-6-astra"));
    assert_eq!(calls[0]["prompt"], calls[1]["prompt"]);
    let limits: Value = serde_json::from_str(
        &fs::read_to_string(repo.0.join(".ralph/provider-limits.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(limits["anthropic"]["allow_failover"], true);
    assert_eq!(limits["openai"]["retry_at"], 0);
    let log = fs::read_to_string(repo.0.join(".ralph/run.log")).unwrap();
    assert!(log.contains("provider failover: claude / claude-fable-5-1 → codex / gpt-6-astra"));
    assert!(!log.contains("limit backoff:"));
}

#[test]
fn failover_keeps_one_shot_and_explicit_backend_and_drops_foreign_flags() {
    let repo = Repo::new();
    repo.config("backend = 'claude'\nextra_args = ['--allowedTools', 'Read', '--effort', 'max']\n");
    success(&repo.run(&["model", "claude-fable-5"], "complete"));
    success(&repo.run(&["--once"], "claude-depleted"));
    let calls = repo.calls();
    assert!(has_pair(&calls[1], "--model", "gpt-6-astra"));
    assert!(has_pair(
        &calls[1],
        "-c",
        "model_reasoning_effort=\"xhigh\""
    ));
    assert!(!calls[1]["args"]
        .as_array()
        .unwrap()
        .contains(&Value::from("--allowedTools")));
}

#[test]
fn cooldown_routes_subsequent_workers_and_helpers() {
    let repo = Repo::new();
    repo.backlog();
    repo.config("model = 'opus'\n");
    let output = repo
        .command(&["--max-iterations", "2"])
        .env("TEST_AGENT_MODE", "claude-depleted")
        .env("TEST_SUCCESS_MODE", "review")
        .output()
        .unwrap();
    success(&output);
    let calls = repo.calls();
    assert_eq!(calls.len(), 5, "{calls:?}");
    assert_eq!(calls[0]["agent"], "claude");
    assert!(calls[1..].iter().all(|c| c["agent"] == "codex"));
    assert!(has_pair(&calls[2], "--model", "gpt-5.6-terra"));
    assert!(has_pair(&calls[3], "--model", "gpt-5.6-sol"));
}

#[test]
fn both_depleted_providers_back_off_without_thrashing() {
    let repo = Repo::new();
    repo.config("limit_wait = 0\nfailover_cooldown = 1\nescalate_after = 1\nabort_after = 1\n");
    success(&repo.run(&["--once"], "both-depleted"));
    assert_eq!(repo.calls().len(), 3);
    let log = fs::read_to_string(repo.0.join(".ralph/run.log")).unwrap();
    assert_eq!(log.matches("provider failover:").count(), 1);
    assert!(log.contains("limit backoff"));
    assert!(!log.contains("ABORTED"));
}

#[test]
fn opt_out_missing_cli_and_usd_budget_keep_existing_limit_retry() {
    for reason in ["disabled", "missing", "budget"] {
        let repo = Repo::new();
        let mut config = "limit_wait = 0\n".to_string();
        match reason {
            "disabled" => config.push_str("provider_failover = false\n"),
            "missing" => fs::remove_file(repo.0.join("bin/codex")).unwrap(),
            "budget" => config.push_str("max_cost_usd = 2.0\n"),
            _ => unreachable!(),
        }
        repo.config(&config);
        let out = repo
            .command(&["--once"])
            .env("TEST_AGENT_MODE", "claude-depleted")
            .env("TEST_DEPLETED_CALLS", "1")
            .output()
            .unwrap();
        success(&out);
        assert!(repo.calls().iter().all(|c| c["agent"] == "claude"));
    }
}

#[test]
fn custom_failover_pair_overrides_the_default() {
    let repo = Repo::new();
    repo.config("[failover_models]\nopus = 'gpt-custom'\n");
    success(&repo.run(&["--model", "opus", "--once"], "claude-depleted"));
    assert!(has_pair(&repo.calls()[1], "--model", "gpt-custom"));
}

#[test]
fn standalone_helper_fails_over_once() {
    let repo = Repo::new();
    fs::write(repo.0.join(".ralph/run.log"), "a recurring failure").unwrap();
    success(&repo.run(&["learn"], "claude-depleted"));
    let calls = repo.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1]["agent"], "codex");
}

#[test]
fn message_failover_pins_the_successful_provider_and_keeps_old_state_on_failure() {
    let repo = Repo::new();
    success(&repo.run(&["msg", "--model", "opus", "hello"], "review"));
    let old = fs::read_to_string(repo.0.join(".ralph/msg-session")).unwrap();
    let out = repo
        .command(&["msg", "follow up"])
        .env("TEST_AGENT_MODE", "both-depleted")
        .env("TEST_DEPLETED_CALLS", "100")
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
    assert_eq!(
        fs::read_to_string(repo.0.join(".ralph/msg-session")).unwrap(),
        old
    );
    success(&repo.run(&["msg", "follow up"], "claude-depleted"));
    assert_eq!(
        fs::read_to_string(repo.0.join(".ralph/msg-backend")).unwrap(),
        "codex\n"
    );
    assert_eq!(
        fs::read_to_string(repo.0.join(".ralph/msg-model")).unwrap(),
        "gpt-5.6-sol\n"
    );
    success(&repo.run(&["msg", "continue"], "review"));
    let calls = repo.calls();
    assert!(has_pair(
        calls.last().unwrap(),
        "resume",
        "11111111-2222-3333-4444-555555555555"
    ));
}

#[test]
fn both_provider_limits_retry_the_first_to_reset() {
    for (claude, codex, expected) in [("1", "4", "claude"), ("4", "1", "codex")] {
        let repo = Repo::new();
        repo.config("limit_wait = 30\nlimit_wait_max = 30\nfailover_cooldown = 60\nescalate_after = 1\nabort_after = 1\n");
        let out = repo
            .command(&["--once"])
            .env("TEST_AGENT_MODE", "both-depleted")
            .env(
                "TEST_CLAUDE_LIMIT_MESSAGE",
                format!("usage limit; try again in {claude} seconds"),
            )
            .env(
                "TEST_CODEX_LIMIT_MESSAGE",
                format!("usage limit; try again in {codex} seconds"),
            )
            .output()
            .unwrap();
        success(&out);
        let calls = repo.calls();
        assert_eq!(calls.len(), 3, "{calls:?}");
        assert_eq!(calls[0]["agent"], "claude");
        assert_eq!(calls[1]["agent"], "codex");
        assert_eq!(calls[2]["agent"], expected);
        let log = fs::read_to_string(repo.0.join(".ralph/run.log")).unwrap();
        assert!(log.contains("from provider output"));
        assert!(log.contains("limit backoff: waiting until"));
        assert!(!log.contains("ABORTED"));
        let limits: Value = serde_json::from_str(
            &fs::read_to_string(repo.0.join(".ralph/provider-limits.json")).unwrap(),
        )
        .unwrap();
        let (available, blocked) = if expected == "claude" {
            ("anthropic", "openai")
        } else {
            ("openai", "anthropic")
        };
        assert_eq!(limits[available]["retry_at"], 0);
        assert!(limits[blocked]["retry_at"].as_i64().unwrap() > 0);
    }
}

#[test]
fn limit_wait_respects_duration_budget_and_survives_restart() {
    let repo = Repo::new();
    let out = repo
        .command(&["--max-duration", "1s"])
        .env("TEST_AGENT_MODE", "both-depleted")
        .env(
            "TEST_CLAUDE_LIMIT_MESSAGE",
            "usage limit; resets in 60 seconds",
        )
        .env(
            "TEST_CODEX_LIMIT_MESSAGE",
            "usage limit; resets in 120 seconds",
        )
        .output()
        .unwrap();
    success(&out);
    assert_eq!(repo.calls().len(), 2);
    let persisted = fs::read_to_string(repo.0.join(".ralph/provider-limits.json")).unwrap();
    let limits: Value = serde_json::from_str(&persisted).unwrap();
    assert!(
        limits["openai"]["retry_at"].as_i64().unwrap()
            > limits["anthropic"]["retry_at"].as_i64().unwrap()
    );
    success(&repo.run(&["--max-duration", "1s"], "complete"));
    assert_eq!(
        repo.calls().len(),
        2,
        "restart must not probe blocked providers"
    );
    assert_eq!(
        fs::read_to_string(repo.0.join(".ralph/provider-limits.json")).unwrap(),
        persisted
    );
}

#[test]
fn explicit_rate_reset_is_honored_with_failover_disabled() {
    let repo = Repo::new();
    repo.config("provider_failover = false\nlimit_wait = 0\nlimit_wait_max = 0\n");
    let started = std::time::Instant::now();
    let out = repo
        .command(&["--model", "gpt-test", "--once"])
        .env("TEST_AGENT_MODE", "codex-depleted")
        .env("TEST_DEPLETED_CALLS", "1")
        .env("TEST_CODEX_LIMIT_MESSAGE", "rate limit; Retry-After: 1")
        .output()
        .unwrap();
    success(&out);
    assert!(started.elapsed() >= std::time::Duration::from_secs(1));
    let calls = repo.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|call| call["agent"] == "codex"));
}

#[test]
fn stop_interrupts_a_long_limit_wait() {
    let repo = Repo::new();
    let mut child = repo
        .command(&["--provider-failover", "false"])
        .env("TEST_AGENT_MODE", "claude-depleted")
        .env("TEST_CLAUDE_LIMIT_MESSAGE", "usage limit; resets in 1 hour")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !repo.0.join(".ralph/provider-limits.json").exists() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("loop never recorded the limit");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    fs::write(repo.0.join(".ralph/STOP"), "stop\n").unwrap();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("limit wait ignored STOP");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(repo.calls().len(), 1);
}

#[test]
fn reset_hint_on_stderr_supplements_a_generic_error_envelope() {
    let repo = Repo::new();
    repo.config("provider_failover = false\nlimit_wait = 0\n");
    let out = repo
        .command(&["--max-duration", "1s"])
        .env("TEST_AGENT_MODE", "claude-depleted")
        .env("TEST_CLAUDE_LIMIT_MESSAGE", "rate limit exceeded")
        .env("TEST_LIMIT_STDERR", "Try again in 1 hour")
        .output()
        .unwrap();
    success(&out);
    assert_eq!(repo.calls().len(), 1);
    let limits: Value = serde_json::from_str(
        &fs::read_to_string(repo.0.join(".ralph/provider-limits.json")).unwrap(),
    )
    .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(limits["anthropic"]["retry_at"].as_i64().unwrap() > now + 3500);
    let log = fs::read_to_string(repo.0.join(".ralph/run.log")).unwrap();
    assert!(log.contains("from provider output"));
}

#[test]
fn exclusive_models_retry_only_the_selected_provider() {
    for (model, mode, cli, concrete) in [
        ("!astra", "codex-depleted", "codex", "gpt-6-astra"),
        ("!fable", "claude-depleted", "claude", "claude-fable-5-1"),
    ] {
        for source in ["flag", "override", "backlog", "mapping"] {
            let repo = Repo::new();
            let mut config = "limit_wait = 0\n".to_string();
            let mut args = vec!["--once"];
            match source {
                "flag" => args.extend(["--model", model]),
                "override" => success(&repo.run(&["model", model], "complete")),
                "backlog" => fs::write(repo.0.join(".ralph/BACKLOG.md"), format!(
                    "<!-- ralph-backlog: v2 -->\n- [ ] **1 — Task.** {model} — implement.\n  Verify: `true` exits 0.\n"
                )).unwrap(),
                "mapping" => config.push_str(&format!("[tier_models]\nsonnet = '{model}'\n")),
                _ => unreachable!(),
            }
            repo.config(&config);
            let out = repo
                .command(&args)
                .env("TEST_AGENT_MODE", mode)
                .env("TEST_DEPLETED_CALLS", "1")
                .output()
                .unwrap();
            success(&out);
            let calls = repo.calls();
            assert!(calls.len() >= 2, "{source}: {calls:?}");
            // A pending task may invoke separately configured helpers after success.
            for call in &calls[..2] {
                assert_eq!(call["agent"], cli, "{source}: {calls:?}");
                assert!(has_pair(call, "--model", concrete), "{source}: {calls:?}");
                assert!(!call["args"]
                    .as_array()
                    .unwrap()
                    .contains(&Value::from("--fallback-model")));
            }
        }
    }
}

#[test]
fn exclusive_message_pin_survives_resume_and_depletion() {
    for (model, cli, concrete) in [
        ("!astra", "codex", "gpt-6-astra"),
        ("!fable", "claude", "claude-fable-5-1"),
    ] {
        let repo = Repo::new();
        success(&repo.run(&["msg", "--model", model, "hello"], "review"));
        let session = fs::read_to_string(repo.0.join(".ralph/msg-session")).unwrap();
        let out = repo.run(&["msg", "continue"], "both-depleted");
        assert!(!out.status.success());
        let calls = repo.calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls
            .iter()
            .all(|c| c["agent"] == cli && has_pair(c, "--model", concrete)));
        assert_eq!(
            fs::read_to_string(repo.0.join(".ralph/msg-model"))
                .unwrap()
                .trim(),
            model
        );
        assert_eq!(
            fs::read_to_string(repo.0.join(".ralph/msg-session")).unwrap(),
            session
        );
    }
}

#[test]
fn exclusive_helper_does_not_fail_over() {
    for model in ["!fable", "!astra"] {
        let repo = Repo::new();
        repo.config(&format!("synth_model = '{model}'\n"));
        fs::write(repo.0.join(".ralph/run.log"), "a recurring failure").unwrap();
        let _ = repo.run(&["learn"], "both-depleted");
        assert_eq!(repo.calls().len(), 1);
    }
}

#[test]
fn exclusive_models_cannot_be_replaced_by_extra_cli_model_flags() {
    for model in ["!fable", "!astra"] {
        let repo = Repo::new();
        repo.config("extra_args = ['--model', 'other', '--model=other', '-mother', '--fallback-model', 'sonnet', '--fallback-model=haiku']\n");
        success(&repo.run(&["--model", model, "--once"], "complete"));
        let calls = repo.calls();
        let args = calls[0]["args"].as_array().unwrap();
        assert_eq!(
            args.iter()
                .filter(|a| **a == Value::from("--model"))
                .count(),
            1
        );
        assert!(!args.iter().any(|a| a.as_str().unwrap().contains("other")
            || a.as_str().unwrap().contains("fallback-model")));
    }
}
