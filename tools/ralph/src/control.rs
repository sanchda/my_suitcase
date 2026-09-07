//! The control loop: spawn each iteration, consume its stream, classify the
//! outcome, apply backoff, and drive no-progress escalation and budgets.
//!
//! The thrash tracker ([`Thrash`]) is a pure state machine tested in isolation;
//! the loop wires it to real subprocesses, git, and the runtime dir.

use crate::backend::{self, Backend};
use crate::classify::{classify, Class};
use crate::config::Config;
use crate::context;
use crate::limits::{Limits, ProviderLimit};
use crate::notify;
use crate::state::State;
use crate::stream::{self, IterStatus, ResultEnvelope};
use crate::{curate, git, inbox, judge, ledger, supervisor, synth, R};
use chrono::Utc;
use std::collections::HashSet;
use std::io::{BufReader, Write};
use std::os::raw::c_int;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// What an iteration achieved, from the thrash tracker's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// A code iteration that advanced HEAD — real progress.
    Made,
    /// A declared productive non-code pass (plan/review): excluded from the streak.
    Excluded,
    /// No progress: code iteration with no commit, or a transient/timeout retry.
    NoProgress,
    /// The iteration declared itself hard-blocked (STATUS=blocked): it needs a
    /// human and a fresh identical iteration will re-block, so escalating is
    /// pointless. Halt after a couple of these instead of spinning.
    Blocked,
}

/// What the loop should do next, after recording a verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Continue,
    /// Force this model on the next attempt (escalation).
    Escalate(String),
    /// Halt: no progress after too many iterations.
    Abort(String),
}

/// Pure no-progress tracker: counts consecutive unproductive iterations and
/// decides when to escalate the model tier and when to give up.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Thrash {
    escalate_after: u32,
    abort_after: u32,
    ladder: Vec<String>,
    tier_models: std::collections::BTreeMap<String, String>,
    streak: u32,
    blocked_streak: u32,
    escalation_idx: Option<usize>,
    target_key: String,
    attempts: u32,
    non_code: u32,
    base_revision: Option<String>,
}

/// Consecutive self-declared `blocked` passes before the loop gives up. A hard
/// block needs a human, so we only wait one iteration to confirm it wasn't a
/// fluke rather than burning the full no-progress budget (and never escalate).
const BLOCKED_ABORT_AFTER: u32 = 2;

impl Thrash {
    pub fn new(cfg: &Config) -> Self {
        Thrash {
            escalate_after: cfg.escalate_after,
            abort_after: cfg.abort_after,
            ladder: cfg.escalation_ladder.clone(),
            tier_models: cfg.tier_models.clone(),
            streak: 0,
            blocked_streak: 0,
            escalation_idx: None,
            target_key: String::new(),
            attempts: 0,
            non_code: 0,
            base_revision: None,
        }
    }

    fn load(cfg: &Config) -> Self {
        let mut fresh = Self::new(cfg);
        if let Ok(text) = std::fs::read_to_string(cfg.dir.join("thrash.json")) {
            if let Ok(old) = serde_json::from_str::<Self>(&text) {
                fresh.streak = old.streak;
                fresh.blocked_streak = old.blocked_streak;
                fresh.escalation_idx = old.escalation_idx.filter(|i| *i < fresh.ladder.len());
                fresh.target_key = old.target_key;
                fresh.attempts = old.attempts;
                fresh.non_code = old.non_code;
                fresh.base_revision = old.base_revision;
            }
        }
        fresh
    }

    fn select(&mut self, cfg: &Config, resolved: &context::IterationContext) {
        let key = format!(
            "{}:{}",
            resolved.task_id.as_deref().unwrap_or("@complete"),
            crate::runtime::fingerprint(&resolved.contract)
        );
        if self.target_key != key {
            *self = Self::new(cfg);
            self.target_key = key;
            self.base_revision = git::head(Path::new("."));
        }
    }

    fn save(&self, cfg: &Config) -> R<()> {
        crate::runtime::write_json(&cfg.dir.join("thrash.json"), self)
    }

    /// The model currently forced by escalation, if any.
    pub fn forced_model(&self) -> Option<String> {
        self.escalation_idx.map(|i| self.ladder[i].clone())
    }

    /// Record an iteration's verdict. `resolved_model` is the tier that ran, used
    /// to compute the next escalation step.
    pub fn record(&mut self, v: Verdict, resolved_model: &str) -> Action {
        match v {
            Verdict::Made => {
                self.streak = 0;
                self.blocked_streak = 0;
                self.escalation_idx = None;
                Action::Continue
            }
            Verdict::Excluded => {
                self.blocked_streak = 0;
                Action::Continue
            }
            Verdict::Blocked => {
                self.blocked_streak += 1;
                if self.blocked_streak >= BLOCKED_ABORT_AFTER {
                    return Action::Abort(format!(
                        "hard-blocked for {} consecutive iterations — needs human intervention",
                        self.blocked_streak
                    ));
                }
                Action::Continue
            }
            Verdict::NoProgress => {
                self.blocked_streak = 0;
                self.streak += 1;
                if self.streak >= self.abort_after {
                    let top = self
                        .forced_model()
                        .unwrap_or_else(|| resolved_model.to_string());
                    return Action::Abort(format!(
                        "no progress after {} iterations (escalated to {top})",
                        self.streak
                    ));
                }
                if self.streak >= self.escalate_after
                    && !resolved_model.starts_with('!')
                    && !self
                        .tier_models
                        .get(resolved_model)
                        .is_some_and(|m| m.starts_with('!'))
                {
                    let cur = self
                        .escalation_idx
                        .or_else(|| self.ladder.iter().position(|m| m == resolved_model))
                        .or_else(|| {
                            let mut matches = self
                                .ladder
                                .iter()
                                .enumerate()
                                .filter(|(_, tier)| {
                                    self.tier_models
                                        .get(*tier)
                                        .is_some_and(|m| m == resolved_model)
                                })
                                .map(|(index, _)| index);
                            matches.next().filter(|_| matches.next().is_none())
                        })
                        // Unmapped or ambiguous concrete models start at medium effort.
                        .or_else(|| self.ladder.iter().position(|m| m == "sonnet"))
                        .unwrap_or(0);
                    let next = (cur + 1).min(self.ladder.len() - 1);
                    self.escalation_idx = Some(next);
                    return Action::Escalate(self.ladder[next].clone());
                }
                Action::Continue
            }
        }
    }
}

pub fn preview_model(cfg: &Config, resolved: &context::IterationContext) -> String {
    let mut thrash = Thrash::load(cfg);
    thrash.select(cfg, resolved);
    let one_shot = std::fs::read_to_string(cfg.dir.join("MODEL"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| backend::valid_model(s));
    choose_model(
        cfg,
        thrash.forced_model().or(one_shot),
        resolved.model_hint.as_deref(),
    )
    .model
}

/// The tier an iteration runs on, plus a log line when the leaf's own `@tier`
/// decoration had to be discarded.
struct ModelChoice {
    model: String,
    note: Option<String>,
}

/// Resolve the model for one iteration. An exclusive task declaration takes
/// precedence; otherwise `override_model` is the escalation / one-shot decision
/// and `hint` is the leaf's model decoration.
///
/// A decoration naming a real tier that the operator left off
/// `escalation_ladder` is valid schema, so lint passes it — lint cannot see
/// config. Routing is the only place that discrepancy is visible, so it reports
/// the drop rather than quietly running the task on the default model.
fn choose_model(cfg: &Config, override_model: Option<String>, hint: Option<&str>) -> ModelChoice {
    // A task's exclusive declaration also survives a persisted escalation.
    if let Some(model) = hint.filter(|h| h.starts_with('!')) {
        return ModelChoice {
            model: model.into(),
            note: None,
        };
    }
    if let Some(model) = override_model {
        return ModelChoice { model, note: None };
    }
    let hint = hint.map(str::trim).filter(|h| !h.is_empty());
    match hint {
        Some(tier)
            if !backend::is_tier(tier) || cfg.escalation_ladder.iter().any(|t| t == tier) =>
        {
            ModelChoice {
                model: tier.to_string(),
                note: None,
            }
        }
        Some(tier) => ModelChoice {
            model: cfg.model.clone(),
            note: Some(format!(
                "  ⚠ task declares @{tier}, absent from escalation_ladder [{}] → running {}",
                cfg.escalation_ladder.join(", "),
                cfg.model
            )),
        },
        None => ModelChoice {
            model: cfg.model.clone(),
            note: None,
        },
    }
}

/// Format the end-of-iteration webhook report. The perf fields come from the
/// result envelope and are omitted when it's absent.
fn iteration_report(
    iter: u64,
    max_iterations: u64,
    pending: usize,
    env: Option<&ResultEnvelope>,
    cost: f64,
    summary: &str,
) -> String {
    let head = if max_iterations > 0 {
        format!("iter {iter}/{max_iterations}")
    } else {
        format!("iter {iter} (~{pending} pending)")
    };
    let mut s = format!("✅ **{head}** — ${cost:.4}");
    if let Some(e) = env.filter(|e| e.duration_ms > 0) {
        let total = e.duration_ms as f64 / 1000.0;
        let api = e.duration_api_ms as f64 / 1000.0;
        let tools = e.duration_ms.saturating_sub(e.duration_api_ms) as f64 / 1000.0;
        s.push_str(&format!(
            " · {} out / {} cache tok · {} turns · ⏱ {total:.0}s (api {api:.0}s / tools {tools:.0}s)",
            human_tokens(e.output_tokens),
            human_tokens(e.cache_read_input_tokens),
            e.num_turns,
        ));
    }
    s.push_str(&format!(" — {summary}"));
    s
}

/// A compact "where this run ended" suffix for terminal webhook messages, so the
/// channel shows the full scope of the run at its stopping point.
fn run_scope(iter: u64, cost_total: f64, elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    let dur = if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}m{:02}s", s / 60, s % 60)
    };
    format!("iter {iter} · ${cost_total:.2} this run · {dur}")
}

/// Compact token count for webhook lines: `512`, `4.2k`, `1.3M`.
fn human_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Capped exponential backoff: 0 → base, else min(cur*2, cap).
pub fn next_backoff(cur: u64, base: u64, cap: u64) -> u64 {
    let n = if cur == 0 {
        base
    } else {
        cur.saturating_mul(2)
    };
    n.min(cap)
}

/// Result of running a single iteration.
struct Ran {
    envelope: Option<ResultEnvelope>,
    killed: bool,
}

/// Live iteration progress the heartbeat thread reads; the stream reader keeps it
/// fresh so a slow webhook POST off-thread never stalls stream consumption.
#[derive(Default, Clone)]
struct HbSnapshot {
    out_tokens: u64,
    events: u64,
    tool: Option<String>,
}

/// The running worker's pid, so the SIGTERM handler can reach it.
static WORKER_PID: AtomicU32 = AtomicU32::new(0);

/// Helpers and verification commands share the force-stop process registry.
pub struct ActiveProcess(u32);
impl ActiveProcess {
    pub fn new(pid: u32) -> Self {
        WORKER_PID.store(pid, Ordering::SeqCst);
        Self(pid)
    }
}
impl Drop for ActiveProcess {
    fn drop(&mut self) {
        kill_group(self.0);
        let _ = WORKER_PID.compare_exchange(self.0, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
}

/// Tear down `claude` before dying: it leads its own session (see `run_one`), so
/// no signal aimed at ralph reaches it and skipping this orphans the whole tree.
/// Then die by the signal, which is what the supervisor classifies on.
extern "C" fn terminate(sig: c_int) {
    // Only `kill` runs before the re-raise — everything here is async-signal-safe.
    kill_group(WORKER_PID.load(Ordering::SeqCst));
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

/// Apply queued backlog mutations. Never fatal: a rejected request is parked in
/// `inbox/rejected/` and reported, not allowed to take the loop down.
fn drain_inbox(cfg: &Config, state: &State, notifier: &Option<notify::Notifier>) {
    match inbox::drain(&cfg.dir, &cfg.backlog) {
        Ok(outcome) => {
            for line in &outcome.applied {
                state.log(&format!("  📥 inbox: {line}"));
            }
            for line in &outcome.rejected {
                state.log(&format!("  ⚠ inbox rejected {line}"));
                notify::notify(
                    notifier,
                    &format!("⚠️ **ralph inbox rejected** — {line} (kept in `inbox/rejected/`)"),
                );
            }
        }
        Err(e) => state.log(&format!("  ⚠ inbox drain failed: {e}")),
    }
}

/// Run the whole loop. Returns the process exit code.
pub fn run(cfg: &Config) -> R<i32> {
    // First thing, so a `ralph stop --now` forwarded here can never find the
    // supervisor's forwarding handler still installed in this process.
    supervisor::install_handler(libc::SIGTERM, terminate);
    let previous = crate::runtime::read(&cfg.dir);
    let mut run = crate::runtime::Run::start(cfg)?;
    if let Some(old) = previous {
        run.record.last_accepted_revision = old.last_accepted_revision;
        if old.phase == "interrupted" {
            if let (Some(task), Some(evidence)) = (old.task, old.artifacts) {
                if !evidence.join("accepted.json").exists() {
                    reject_checkoff(cfg, &task)?;
                    let contract =
                        std::fs::read_to_string(evidence.join("contract.md")).unwrap_or_default();
                    crate::runtime::feedback(&cfg.dir, &task, &contract,
                        "The previous process was interrupted before acceptance. Inspect its partial work and evidence before requesting completion again.", &evidence)?;
                }
            }
        }
    }
    let result = run_loop(cfg, &mut run);
    if run.record.terminal_reason.is_none() {
        let reason = match &result {
            Err(e) => format!("error: {e}"),
            Ok(0) => "stopped".into(),
            Ok(_) => "aborted".into(),
        };
        run.finish(&reason)?;
    }
    result
}

fn run_loop(cfg: &Config, run: &mut crate::runtime::Run) -> R<i32> {
    if !cfg.prompt.exists() {
        return Err(format!("prompt file not found: {}", cfg.prompt.display()).into());
    }
    let state = State::open(&cfg.dir)?;
    let repo = Path::new(".");
    git::write_baseline(repo, &state.baseline_path());

    state.log(&format!(
        "=== ralph start (model={} effort={} fallback={} marker={} max_iter={} max_cost={} max_dur={}s yolo={}) ===",
        cfg.model,
        cfg.effort,
        if cfg.fallback_model.is_empty() { "none" } else { &cfg.fallback_model },
        cfg.marker,
        cfg.max_iterations,
        cfg.max_cost_usd,
        cfg.max_duration,
        cfg.yolo,
    ));

    let mut thrash = Thrash::load(cfg);
    let mut iter = state.iteration();
    run.record.iteration = iter;
    let mut twait = 0u64;
    let mut failover = Limits::load(&cfg.dir).unwrap_or_else(|e| {
        state.log(&format!("  ⚠ could not load provider limits: {e}"));
        Limits::default()
    });
    let mut retry_model = None;
    let mut cost_total = 0.0f64;
    let mut seen_context_warnings = HashSet::new();
    let start = Instant::now();

    let notifier = notify::Notifier::new(&cfg.discord_webhook);
    notify::notify(
        &notifier,
        &format!(
            "🟢 **ralph started** — model `{}`, from iter {}",
            cfg.model, iter
        ),
    );

    loop {
        if state.stop_requested() {
            state.log("STOP file present → halting");
            notify::notify(
                &notifier,
                &format!(
                    "⏹️ **ralph halted** — stop requested, honored after the current job · {}",
                    run_scope(iter, cost_total, start.elapsed())
                ),
            );
            state.clear_stop();
            run.finish("stop requested")?;
            break;
        }
        if cfg.max_iterations > 0 && iter >= cfg.max_iterations {
            run.finish("iteration limit")?;
            state.log(&format!(
                "max iterations ({}) reached → halting",
                cfg.max_iterations
            ));
            notify::notify(
                &notifier,
                &format!(
                    "⏹️ **ralph halted** — max iterations ({}) reached",
                    cfg.max_iterations
                ),
            );
            break;
        }
        if cfg.max_cost_usd > 0.0 && cost_total >= cfg.max_cost_usd {
            run.finish("worker cost limit")?;
            state.log(&format!(
                "cost budget reached (${:.4} ≥ ${:.4}) → halting",
                cost_total, cfg.max_cost_usd
            ));
            notify::notify(
                &notifier,
                &format!(
                    "⏹️ **ralph halted** — cost budget ${:.2} reached",
                    cfg.max_cost_usd
                ),
            );
            break;
        }
        // Unlike max_cost_usd this reads the ledger, so a --restart relaunch
        // does not hand the loop a fresh allowance.
        if cfg.budget_usd > 0.0 {
            let spent = ledger::spend_since(&cfg.dir, cfg.budget_window);
            if spent >= cfg.budget_usd {
                run.finish("persisted spend limit")?;
                let window = match cfg.budget_window {
                    0 => "all time".to_string(),
                    secs => format!("the last {secs}s"),
                };
                state.log(&format!(
                    "ledger budget reached (${spent:.4} ≥ ${:.4} over {window}) → halting",
                    cfg.budget_usd
                ));
                notify::notify(
                    &notifier,
                    &format!(
                        "⏹️ **ralph halted** — ledger budget ${:.2} reached over {window}",
                        cfg.budget_usd
                    ),
                );
                break;
            }
        }
        if cfg.max_duration > 0 && start.elapsed().as_secs() >= cfg.max_duration {
            run.finish("duration limit")?;
            state.log(&format!(
                "wall-clock budget ({}s) reached → halting",
                cfg.max_duration
            ));
            notify::notify(
                &notifier,
                &format!(
                    "⏹️ **ralph halted** — wall-clock budget ({}s) reached",
                    cfg.max_duration
                ),
            );
            break;
        }

        // Before routing, so queued work is visible to leaf selection — and on
        // every iteration, whatever the previous turn did.
        drain_inbox(cfg, &state, &notifier);

        let next = iter + 1;
        let resolved = context::load(&cfg.backlog, &cfg.progress);
        if resolved.has_errors() {
            let errors = resolved.errors().collect::<Vec<_>>().join("\n  ");
            return Err(format!(
                "backlog schema is invalid:\n  {errors}\nrun `ralph lint` for details"
            )
            .into());
        }
        thrash.select(cfg, &resolved);
        run.record.task = Some(resolved.task_id.as_deref().unwrap_or("@complete").into());
        run.record.task_attempts = thrash.attempts;
        if cfg.task_attempt_limit > 0 && thrash.attempts >= cfg.task_attempt_limit {
            run.finish(&format!(
                "task attempt limit: {} after {} attempts",
                resolved.task_id.as_deref().unwrap_or("@complete"),
                thrash.attempts
            ))?;
            return Ok(1);
        }
        // Model precedence: escalation > a one-shot `.ralph/MODEL` override the
        // agent wrote > the resolved leaf's own `@tier` decoration > default.
        let retry = retry_model.take();
        let choice = choose_model(
            cfg,
            thrash
                .forced_model()
                .or_else(|| state.take_model(&cfg.escalation_ladder))
                .or(retry),
            resolved.model_hint.as_deref(),
        );
        let model = choice.model;
        let primary = backend::resolve(cfg, &model);
        let now = Utc::now().timestamp();
        let until = failover.wait_until(primary.backend, alternate_usable(cfg, &model), now);
        if until > now {
            retry_model = Some(model.clone());
            let message = format!(
                "  limit backoff: waiting until {} ({}s), then retry iter {next}",
                reset_label(until),
                until - now
            );
            state.log(&message);
            state.write_live_status(&format!("{message}\n"));
            run.record.retry_at = Some(until);
            run.phase("waiting_for_quota")?;
            wait_for_limit(cfg, &state, until, start);
            continue;
        }
        let routed = route_config(cfg, &model, &failover);
        let selection = backend::resolve(&routed, &model);
        let actual_model = selection.model.as_deref().unwrap_or(&model);
        // Learnings ride the stable base: they change rarely, so the
        // prompt-cache prefix survives across iterations.
        let iteration_prompt = context::full_prompt(cfg, &resolved)?;
        let head_before = git::head(repo);
        let tree_before = git::tree(repo);
        let branch_before = git::branch(repo);
        let backlog_before = std::fs::read_to_string(&cfg.backlog)?;
        let task_id = resolved.task_id.as_deref().unwrap_or("@complete");
        let policy = cfg.acceptance.get(task_id).cloned().unwrap_or_default();
        let evidence = run.begin_attempt(
            &routed,
            next,
            Some(task_id),
            actual_model,
            selection.backend.executable(),
            &iteration_prompt,
        )?;
        std::fs::write(evidence.join("contract.md"), &resolved.contract)?;
        let target = resolved.target.as_deref().unwrap_or("completion audit");
        state.log(&format!(
            "iter {next} → {actual_model} (effort={}, target={target}, requested={model})",
            effort_for(&routed, &model).unwrap_or_else(|| "inherited".into()),
        ));
        if let Some(note) = &choice.note {
            state.log(note);
        }
        // Post the leaf's title too, so the channel says what it's working on.
        let task_label = match &resolved.target_title {
            Some(title) => format!("{target} — {title}"),
            None => target.to_string(),
        };
        notify::notify(
            &notifier,
            &format!("▶️ **iter {next}** → `{actual_model}` · {task_label}"),
        );
        for warning in resolved.warnings() {
            if seen_context_warnings.insert(context_warning_key(warning)) {
                state.log(&format!("  ⚠ {warning}"));
            }
        }
        let ran = run_one(&routed, &state, next, &model, &iteration_prompt)?;

        let (class, cost, text) = match &ran.envelope {
            Some(env) => {
                state.write_last_result(&env.raw);
                let c = classify(env.is_error, env.api_error_status, &env.result);
                (c, env.total_cost_usd, env.result.clone())
            }
            // No envelope: crash, kill, or empty output → transient.
            None => (Class::Transient, 0.0, String::new()),
        };
        cost_total += cost;
        run.record.worker_cost_usd = cost_total;
        run.record.cost_unreported |= selection.backend == Backend::Codex;
        std::fs::write(evidence.join("worker-summary.md"), &text)?;
        if let Some(env) = &ran.envelope {
            crate::runtime::write_json(&evidence.join("worker-result.json"), &env.raw)?;
        }
        if let Ok(log) = std::fs::read_link(cfg.dir.join("current.log")) {
            std::fs::write(
                evidence.join("worker-log-path.txt"),
                cfg.dir.join(log).display().to_string(),
            )?;
        }
        if class != Class::Limit {
            thrash.attempts += 1;
            run.record.task_attempts = thrash.attempts;
            thrash.save(cfg)?;
            run.save()?;
        }
        if class != Class::Success {
            reject_checkoff(cfg, task_id)?;
            if class != Class::Limit {
                crate::runtime::feedback(
                    &cfg.dir,
                    task_id,
                    &resolved.contract,
                    &format!("Worker did not complete successfully: {class:?}\n{text}"),
                    &evidence,
                )?;
            }
        }

        if let Err(e) = ledger::append(&cfg.dir, next, actual_model, cost) {
            state.log(&format!("  ⚠ could not append to the spend ledger: {e}"));
        }
        if let Some(env) = &ran.envelope {
            if env.duration_ms > 0 {
                let non_api_ms = env.duration_ms.saturating_sub(env.duration_api_ms);
                state.log(&format!(
                    "  perf total={:.1}s api={:.1}s non-api={:.1}s turns={} tokens(in/new-cache/read-cache/out)={}/{}/{}/{}",
                    env.duration_ms as f64 / 1000.0,
                    env.duration_api_ms as f64 / 1000.0,
                    non_api_ms as f64 / 1000.0,
                    env.num_turns,
                    env.input_tokens,
                    env.cache_creation_input_tokens,
                    env.cache_read_input_tokens,
                    env.output_tokens,
                ));
            }
        }

        match class {
            Class::Success => {
                iter = next;
                state.set_iteration(iter)?;
                *failover.get_mut(selection.backend) = ProviderLimit::default();
                save_limits(&failover, cfg, &state);
                twait = 0;
                let snippet: String = text.chars().take(160).collect();
                state.log(&format!(
                    "  ok (${cost:.4}) — {}",
                    snippet.replace('\n', " ")
                ));

                let marker = stream::has_marker(&text, &cfg.marker);
                run.phase("auditing")?;
                let handoff = state.take_handoff(&cfg.escalation_ladder);
                if let Some(m) = handoff.as_ref().and_then(|h| h.model.as_deref()) {
                    state.write_model(m);
                }
                let status = handoff
                    .as_ref()
                    .and_then(|h| h.status.clone())
                    .or_else(|| state.read_status());
                state.clear_status();
                let blocked = handoff
                    .as_ref()
                    .and_then(|h| h.blocked.as_deref())
                    .unwrap_or("needs human intervention (no reason given)");
                let breaches = git::audit_iteration(repo, &branch_before, &head_before, &cfg.dir);
                let mut rejection = breaches
                    .iter()
                    .map(|b| b.message.clone())
                    .collect::<Vec<_>>();
                let mut fatal_breach = breaches.iter().any(|b| b.fatal);
                let advanced = git::advanced_since(repo, &head_before);
                let mut verdict = match status.as_deref() {
                    Some("blocked") => {
                        rejection.push(format!("Blocked: {blocked}"));
                        Verdict::Blocked
                    }
                    Some("plan" | "review") => Verdict::Excluded,
                    _ if advanced => Verdict::Made,
                    _ => Verdict::NoProgress,
                };
                let closing = if task_id == "@complete" {
                    marker
                } else {
                    inbox::has_done(&cfg.dir, task_id) || task_checked(cfg, task_id)
                };
                let checked = closing && rejection.is_empty() && !policy.command.is_empty();
                if checked {
                    run.phase("verifying")?;
                    let receipt = crate::acceptance::check(&policy, task_id, &evidence)?;
                    if !receipt.passed() {
                        rejection.push(crate::acceptance::failure_text(&receipt, &evidence));
                    }
                }
                let legacy_judge = verdict == Verdict::Made
                    && judge::wants_judgment(cfg, &model)
                    && task_id != "@complete";
                let wants_review = closing && policy.review != crate::acceptance::Review::Off;
                let mut unavailable = false;
                let reviewed = rejection.is_empty() && (legacy_judge || wants_review);
                if reviewed {
                    run.phase("reviewing")?;
                    let contract = if resolved.contract.is_empty() {
                        iteration_prompt.clone()
                    } else {
                        resolved.contract.clone()
                    };
                    match crate::acceptance::review(
                        &routed,
                        &contract,
                        &text,
                        &thrash.base_revision,
                        &evidence,
                    )? {
                        judge::Decision::Pass => state.log("  judge: pass"),
                        judge::Decision::Refuted(reason) => {
                            if policy.review == crate::acceptance::Review::Advisory && !legacy_judge
                            {
                                state.log(&format!("  advisory review: {reason} (recorded; acceptance is unchanged)"));
                            } else {
                                rejection.push(format!("Judge REFUTED: {reason}"));
                            }
                        }
                        judge::Decision::Unavailable => {
                            state.log("  judge: unavailable (not a pass)");
                            if policy.review == crate::acceptance::Review::Required {
                                unavailable = true;
                                rejection.push("Required review unavailable; retry after the reviewer is usable".into());
                            }
                        }
                    }
                }
                // A check command/reviewer is also subject to the branch/history contract.
                if checked || reviewed {
                    for breach in git::audit_iteration(repo, &branch_before, &head_before, &cfg.dir)
                    {
                        fatal_breach |= breach.fatal;
                        if !rejection.contains(&breach.message) {
                            rejection.push(breach.message);
                        }
                    }
                }
                let rejected = !rejection.is_empty();
                if rejected {
                    reject_checkoff(cfg, task_id)?;
                    let reason = rejection.join("\n");
                    state.log(&format!("  acceptance rejected: {reason}"));
                    notify::notify(
                        &notifier,
                        &format!("⚠️ **ralph acceptance rejected** — {task_id}: {reason}"),
                    );
                    crate::runtime::feedback(
                        &cfg.dir,
                        task_id,
                        &resolved.contract,
                        &reason,
                        &evidence,
                    )?;
                    if verdict != Verdict::Blocked {
                        verdict = Verdict::NoProgress;
                    }
                }
                crate::runtime::write_json(
                    &evidence.join("outcome.json"),
                    &serde_json::json!({
                        "task": task_id, "contract": crate::runtime::fingerprint(&resolved.contract),
                        "closing_requested": closing, "rejections": rejection, "revision": git::head(repo),
                    }),
                )?;
                if fatal_breach || unavailable {
                    thrash.save(cfg)?;
                    run.finish(if fatal_breach {
                        "Git contract breach"
                    } else {
                        "required review unavailable"
                    })?;
                    return Ok(1);
                }
                // Reconcile before completion, and only after rejecting failed check-offs.
                drain_inbox(cfg, &state, &notifier);
                let post = context::load(&cfg.backlog, &cfg.progress);
                if !rejected && closing && (task_checked(cfg, task_id) || task_id == "@complete") {
                    run.record.last_accepted_revision = git::head(repo);
                    crate::runtime::write_json(
                        &evidence.join("accepted.json"),
                        &serde_json::json!({"task": task_id, "revision": git::head(repo)}),
                    )?;
                    let _ = std::fs::remove_file(cfg.dir.join("previous-attempt.json"));
                }
                // Completion policies get their own audit turn when the last leaf closes.
                let final_policy_pending =
                    task_id != "@complete" && cfg.acceptance.contains_key("@complete");
                if marker
                    && !rejected
                    && post.is_complete()
                    && !inbox::has_pending(&cfg.dir)
                    && !final_policy_pending
                {
                    newly_dirty_warn(&state, repo);
                    state.log("  marker seen and backlog has no pending task → COMPLETE");
                    archive_backlog(cfg, &state);
                    finish_arc(cfg, &state);
                    let _ = std::fs::remove_file(cfg.dir.join("thrash.json"));
                    let _ = std::fs::remove_file(cfg.dir.join("previous-attempt.json"));
                    run.finish("complete")?;
                    notify::notify(
                        &notifier,
                        &format!(
                            "✅ **ralph COMPLETE** — backlog done · {}",
                            run_scope(iter, cost_total, start.elapsed())
                        ),
                    );
                    break;
                }
                if marker {
                    state.log("  ⚠ completion marker ignored: pending work, rejected acceptance, or final policy remains");
                }
                newly_dirty_warn(&state, repo);
                let backlog_after = std::fs::read_to_string(&cfg.backlog).unwrap_or_default();
                if !rejected && verdict == Verdict::Excluded {
                    if backlog_after == backlog_before && tree_before == git::tree(repo) {
                        thrash.non_code += 1;
                        if thrash.non_code >= 2 {
                            verdict = Verdict::NoProgress;
                            state.log("  repeated non-code pass without a changed plan or product tree → no-progress");
                        }
                    } else {
                        thrash.non_code = 0;
                    }
                } else {
                    thrash.non_code = 0;
                }
                if !rejected
                    && verdict == Verdict::Made
                    && tree_before.is_some()
                    && tree_before == git::tree(repo)
                    && backlog_before == backlog_after
                {
                    verdict = Verdict::NoProgress;
                    state.log("  commit changed neither product tree nor plan → no-progress");
                }
                if thrash.attempts >= 4 && post.task_id == resolved.task_id {
                    state.log(&format!("  task {} still selected after {} attempts; split/reframe if needed (task_attempt_limit is {})", task_id, thrash.attempts, cfg.task_attempt_limit));
                }
                run.phase("handoff")?;
                let doc = crate::backlog::Document::parse(&backlog_after);
                let upcoming = doc.upcoming_leaf_labels(3);
                let prev = std::fs::read_to_string(&cfg.progress).unwrap_or_default();
                let carry =
                    synth::synthesize_with(&text, &upcoming, &prev, |p| synth::run(&routed, p));
                crate::backlog_edit::write_atomic(&cfg.progress, &carry)?;
                if !rejected {
                    let swept = curate::sweep(&cfg.backlog, &cfg.dir.join("archive"));
                    if swept > 0 {
                        state.log(&format!(
                            "  ✂ curated {swept} completed section(s) → archive"
                        ));
                    }
                }
                std::fs::copy(&cfg.backlog, evidence.join("BACKLOG-after.md"))?;
                notify::notify(
                    &notifier,
                    &iteration_report(
                        iter,
                        cfg.max_iterations,
                        doc.pending_leaf_count(),
                        ran.envelope.as_ref(),
                        cost,
                        &snippet.replace('\n', " "),
                    ),
                );
                let abort = apply_verdict(&mut thrash, verdict, &model, &state, &notifier);
                thrash.save(cfg)?;
                run.save()?;
                if abort {
                    run.finish(if verdict == Verdict::Blocked {
                        "repeated human block"
                    } else {
                        "no progress"
                    })?;
                    return Ok(1);
                }
                if cfg.once {
                    run.finish("single iteration finished")?;
                    break;
                }
            }
            Class::Limit => {
                // Preserve one-shot sizing across quota retries; never feed thrash.
                retry_model = Some(model.clone());
                let snippet: String = text.chars().take(160).collect();
                state.log(&format!(
                    "  USAGE/RATE LIMIT — {}",
                    snippet.replace('\n', " ")
                ));
                let now = Utc::now();
                let depleted = crate::classify::depleted(&text);
                let limit = failover.get_mut(selection.backend);
                limit.backoff = next_backoff(limit.backoff, cfg.limit_wait, cfg.limit_wait_max);
                let fallback = if depleted && alternate_usable(cfg, &model) {
                    cfg.failover_cooldown
                } else {
                    limit.backoff
                };
                let parsed = crate::limits::reset_at(&text, now);
                // Saturating arithmetic keeps malformed configuration from wrapping deadlines.
                limit.retry_at = parsed
                    .map(|at| {
                        at.timestamp()
                            .saturating_add(i64::from(at.timestamp_subsec_nanos() > 0))
                    })
                    .unwrap_or_else(|| {
                        now.timestamp()
                            .saturating_add(fallback.min(i64::MAX as u64) as i64)
                    });
                limit.allow_failover = depleted;
                state.log(&format!(
                    "  {} limited until {} ({}); independent provider timer saved",
                    selection.backend.executable(),
                    reset_label(limit.retry_at),
                    if parsed.is_some() {
                        "from provider output"
                    } else {
                        "configured fallback"
                    }
                ));
                save_limits(&failover, cfg, &state);
                let next_cfg = route_config(cfg, &model, &failover);
                let next_selection = backend::resolve(&next_cfg, &model);
                if next_selection.backend != selection.backend
                    && failover.get(next_selection.backend).retry_at <= Utc::now().timestamp()
                {
                    state.log(&format!(
                        "  provider failover: {} / {} → {} / {}; retry iter {next}",
                        selection.backend.executable(),
                        actual_model,
                        next_selection.backend.executable(),
                        next_selection
                            .model
                            .as_deref()
                            .unwrap_or("configured default"),
                    ));
                }
            }
            Class::Transient => {
                let reason = if ran.killed {
                    "killed by per-iteration timeout".to_string()
                } else {
                    let snippet: String = text.chars().take(160).collect();
                    format!("transient — {}", snippet.replace('\n', " "))
                };
                state.log(&format!("  {reason}"));
                // A transient (including a timeout strike) is no-progress.
                let abort =
                    apply_verdict(&mut thrash, Verdict::NoProgress, &model, &state, &notifier);
                thrash.save(cfg)?;
                if abort {
                    run.finish("repeated transient failures")?;
                    return Ok(1);
                }
                twait = next_backoff(twait, cfg.transient_wait, cfg.transient_wait_max);
                state.log(&format!(
                    "  transient backoff: sleeping {twait}s, then retry iter {next}"
                ));
                run.phase("waiting_for_retry")?;
                let deadline = Instant::now() + Duration::from_secs(twait);
                while Instant::now() < deadline && !state.stop_requested() {
                    if cfg.max_duration > 0 && start.elapsed().as_secs() >= cfg.max_duration {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
            Class::Fatal => {
                run.finish(&format!(
                    "fatal provider error: {}",
                    crate::runtime::bounded(&text, 512)
                ))?;
                let snippet: String = text.chars().take(200).collect();
                state.log(&format!(
                    "=== ralph ABORTED (fatal) — {} ===",
                    snippet.replace('\n', " ")
                ));
                return Ok(1);
            }
        }
    }
    Ok(0)
}

fn task_checked(cfg: &Config, id: &str) -> bool {
    let text = std::fs::read_to_string(&cfg.backlog).unwrap_or_default();
    crate::backlog::Document::parse(&text)
        .tasks
        .iter()
        .any(|t| t.id == id && t.checked)
}

fn reject_checkoff(cfg: &Config, id: &str) -> R<()> {
    if id == "@complete" {
        return Ok(());
    }
    // Also discard ancestor closure requests; otherwise a refuted child can be
    // re-closed by a queued parent integration check-off.
    let text = std::fs::read_to_string(&cfg.backlog).unwrap_or_default();
    let doc = crate::backlog::Document::parse(&text);
    let mut index = doc.tasks.iter().position(|t| t.id == id);
    inbox::discard_done(&cfg.dir, id)?;
    while let Some(i) = index {
        inbox::discard_done(&cfg.dir, &doc.tasks[i].id)?;
        index = doc.tasks[i].parent;
    }
    if doc.tasks.iter().any(|t| t.id == id && t.checked) {
        let text = crate::backlog_edit::apply_uncheck(&text, id)?;
        crate::backlog_edit::write_atomic(&cfg.backlog, &text)?;
    }
    Ok(())
}

/// On completion, archive whatever backlog remains. With incremental curation the
/// live file is usually just its header by now. Best-effort; never touches git.
fn archive_backlog(cfg: &Config, state: &State) {
    if !cfg.backlog.exists() {
        return;
    }
    let archive_dir = cfg.dir.join("archive");
    if let Err(e) = std::fs::create_dir_all(&archive_dir) {
        state.log(&format!("  ⚠ could not create archive dir: {e}"));
        return;
    }
    let dest = archive_dir.join(format!("BACKLOG-{}.md", crate::state::timestamp()));
    if rename_or_copy(&cfg.backlog, &dest) {
        state.log(&format!("  archived backlog → {}", dest.display()));
    } else {
        state.log(&format!(
            "  ⚠ could not archive backlog {}",
            cfg.backlog.display()
        ));
    }
}

/// Close out a completed arc so the next backlog starts clean: archive and
/// clear the carry-forward, and reset the iteration counter
/// (`--max-iterations` compares against it absolutely, and post-COMPLETE
/// there is nothing to resume). `.ralph/learnings/` deliberately survives.
/// Best-effort: a finished run is never turned into a failure here.
fn finish_arc(cfg: &Config, state: &State) {
    if let Ok(text) = std::fs::read_to_string(&cfg.progress) {
        if !text.trim().is_empty() {
            let archive_dir = cfg.dir.join("archive");
            if std::fs::create_dir_all(&archive_dir).is_ok() {
                let dest = archive_dir.join(format!("PROGRESS-{}.md", crate::state::timestamp()));
                let _ = std::fs::write(dest, &text);
            }
        }
        let _ = std::fs::write(&cfg.progress, "");
    }
    // A closed arc is the natural boundary for the steering session's context too.
    if let Some(id) = crate::msg::archive_session(&cfg.dir) {
        state.log(&format!("  archived msg session {id}"));
    }
    match state.set_iteration(0) {
        Ok(()) => state.log("  arc closed: carry-forward archived, iteration counter reset"),
        Err(e) => state.log(&format!("  ⚠ could not reset iteration counter: {e}")),
    }
}

/// Move a file, falling back to copy+remove when `rename` crosses filesystems.
fn rename_or_copy(from: &Path, to: &Path) -> bool {
    if std::fs::rename(from, to).is_ok() {
        return true;
    }
    std::fs::copy(from, to).is_ok() && std::fs::remove_file(from).is_ok()
}

/// Apply a verdict to the tracker, logging escalation and returning `true` if the
/// loop should abort.
fn apply_verdict(
    thrash: &mut Thrash,
    v: Verdict,
    model: &str,
    state: &State,
    notifier: &Option<notify::Notifier>,
) -> bool {
    match thrash.record(v, model) {
        Action::Continue => false,
        Action::Escalate(m) => {
            state.log(&format!("  ↑ no-progress streak → escalating model to {m}"));
            notify::notify(
                notifier,
                &format!("⚠️ **ralph** — no progress, escalating model to `{m}`"),
            );
            false
        }
        Action::Abort(reason) => {
            state.log(&format!("=== ralph ABORTED — {reason} ==="));
            notify::notify(notifier, &format!("🔴 **ralph ABORTED** — {reason}"));
            true
        }
    }
}

/// Warn (don't act) if the tracked tree gained new dirt vs. the baseline.
fn newly_dirty_warn(state: &State, repo: &Path) {
    let n = git::newly_dirty(repo, &state.baseline_path());
    if n > 0 {
        state.log(&format!(
            "  ⚠ {n} newly-dirty tracked file(s) — agent may have skipped its commit"
        ));
    }
}

fn reset_label(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|at| at.to_rfc3339())
        .unwrap_or_else(|| timestamp.to_string())
}

fn save_limits(limits: &Limits, cfg: &Config, state: &State) {
    if let Err(e) = limits.save(&cfg.dir) {
        state.log(&format!("  ⚠ could not save provider limits: {e}"));
    }
}

fn alternate_usable(cfg: &Config, model: &str) -> bool {
    let primary = backend::resolve(cfg, model);
    let alternate = backend::counterpart(cfg, model, &primary);
    cfg.provider_failover
        && !primary.exclusive
        && which(alternate.backend.executable()).is_some()
        && backend::check_cost_budget(cfg, &alternate).is_ok()
}

/// Long quota windows must remain responsive to STOP and wall-clock budgets.
fn wait_for_limit(cfg: &Config, state: &State, until: i64, start: Instant) {
    while Utc::now().timestamp() < until {
        if state.stop_requested()
            || (cfg.max_duration > 0 && start.elapsed().as_secs() >= cfg.max_duration)
        {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

/// Route workers and their helpers away from a depleted provider.
fn route_config(cfg: &Config, model: &str, failover: &Limits) -> Config {
    let mut routed = cfg.clone();
    let primary = backend::resolve(cfg, model);
    if cfg.provider_failover
        && !primary.exclusive
        && failover.route(primary.backend, Utc::now().timestamp())
    {
        let alternate = backend::counterpart(cfg, model, &primary);
        if which(alternate.backend.executable()).is_some()
            && backend::check_cost_budget(cfg, &alternate).is_ok()
        {
            routed.failover_from = Some(primary.backend);
            // CLI-specific options must not leak into the other CLI grammar.
            if let Some(effort) = extra_effort(&cfg.extra_args) {
                routed.effort = effort;
            }
            routed.extra_args.clear();
        }
    }
    routed
}

/// Spawn an iteration and collect its result.
fn run_one(cfg: &Config, state: &State, n: u64, model: &str, prompt: &str) -> R<Ran> {
    let log_path = state.new_iter_log(n)?;

    let selection = backend::resolve(cfg, model);
    backend::check_cost_budget(cfg, &selection)?;
    if which(selection.backend.executable()).is_none() {
        return Err(format!("{} CLI not found on PATH", selection.backend.executable()).into());
    }
    state.log(&format!(
        "  backend={} model={}",
        selection.backend.executable(),
        selection.model.as_deref().unwrap_or("configured default")
    ));
    let args = if selection.backend == Backend::Codex {
        let mut args =
            backend::codex_args(&selection, (!cfg.yolo).then_some("workspace-write"), true);
        args.extend(backend::extra_args(cfg, &selection));
        args.push("-".into());
        args
    } else {
        claude_args(cfg, model)
    };

    let mut cmd = Command::new(selection.backend.executable());
    cmd.args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A separate session lets timeout/stop signals kill the worker and its
    // descendants without reaching Ralph or its caller.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    let pid = child.id();
    WORKER_PID.store(pid, Ordering::SeqCst);

    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // Drain stderr into the same raw log (best-effort) on its own thread.
    let stderr_log = log_path.clone();
    let stderr_thread = thread::spawn(move || {
        use std::io::{BufRead, Write};
        let mut diagnostic = String::new();
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(&stderr_log)
            .ok();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Some(f) = &mut log {
                let _ = writeln!(f, "{line}");
            }
            diagnostic.push_str(&line);
            diagnostic.push('\n');
            if diagnostic.len() > 16_384 {
                let mut cut = diagnostic.len() - 16_384;
                while !diagnostic.is_char_boundary(cut) {
                    cut += 1;
                }
                diagnostic.drain(..cut);
            }
        }
        diagnostic
    });

    // Watchdog: kill the child's process group if it outlives the timeout.
    let killed = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let watchdog = if cfg.iteration_timeout > 0 {
        let (killed_w, done_w) = (killed.clone(), done.clone());
        let deadline = cfg.iteration_timeout;
        Some(thread::spawn(move || {
            let ticks = deadline * 10; // poll every 100ms
            for _ in 0..ticks {
                if done_w.load(Ordering::SeqCst) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
            if !done_w.load(Ordering::SeqCst) {
                killed_w.store(true, Ordering::SeqCst);
                kill_group(pid);
            }
        }))
    } else {
        None
    };

    // Feed stdin concurrently, after the watchdog is armed. A child that stops
    // reading a large prompt can no longer block the runner before its timeout.
    let prompt_bytes = prompt.as_bytes().to_vec();
    let prompt_thread = thread::spawn(move || {
        let mut stdin = stdin;
        stdin.write_all(&prompt_bytes)
    });

    // Optional heartbeat: while the turn runs, post live progress (elapsed,
    // output tokens, current tool) to the webhook every heartbeat_interval. Its
    // own thread means a slow POST never stalls stream consumption; it reads a
    // snapshot the stream reader keeps fresh, and stops via `done`.
    let hb_shared = Arc::new(Mutex::new(HbSnapshot::default()));
    let heartbeat = if cfg.heartbeat_interval > 0 && !cfg.discord_webhook.trim().is_empty() {
        let (done_h, shared_h) = (done.clone(), hb_shared.clone());
        let webhook = cfg.discord_webhook.clone();
        let interval = cfg.heartbeat_interval;
        Some(thread::spawn(move || {
            let notifier = notify::Notifier::new(&webhook);
            let start = Instant::now();
            loop {
                // Wait `interval` seconds in 1s ticks so the turn ending stops us
                // promptly rather than after a full interval.
                for _ in 0..interval {
                    if done_h.load(Ordering::SeqCst) {
                        return;
                    }
                    thread::sleep(Duration::from_secs(1));
                }
                if done_h.load(Ordering::SeqCst) {
                    return;
                }
                let snap = shared_h.lock().map(|g| g.clone()).unwrap_or_default();
                let elapsed = start.elapsed().as_secs();
                let tool = snap.tool.map(|t| format!(" · {t}")).unwrap_or_default();
                notify::notify(
                    &notifier,
                    &format!(
                        "⏳ **iter {n}** · {}m{:02}s · ~{} out tok · {} events{tool}",
                        elapsed / 60,
                        elapsed % 60,
                        snap.out_tokens,
                        snap.events,
                    ),
                );
            }
        }))
    } else {
        None
    };

    // Consume the stream on this thread (blocks until EOF / child exit / kill).
    let mut raw = std::fs::OpenOptions::new().append(true).open(&log_path)?;
    let mut status = IterStatus::new(n, selection.model.as_deref().unwrap_or(model));
    state.write_live_status(&status.render());
    let reader = BufReader::new(stdout);
    let hb_emit = hb_shared.clone();
    let envelope = stream::consume(reader, &mut raw, &mut status, |s| {
        state.write_live_status(&s.render());
        if let Ok(mut g) = hb_emit.lock() {
            g.out_tokens = s.out_tokens;
            g.events = s.events;
            g.tool = s.current_tool.clone();
        }
    })?;

    // Sweep descendants before reaping the worker so its process-group ID
    // cannot be reused while cleanup is in progress.
    done.store(true, Ordering::SeqCst);
    #[cfg(unix)]
    kill_group(pid);
    let _ = child.wait();
    // Reaped: the pid can be recycled, so the handler must stop aiming at it.
    WORKER_PID.store(0, Ordering::SeqCst);
    let prompt_result = prompt_thread.join();
    let diagnostic = stderr_thread.join().unwrap_or_default();
    if let Some(w) = watchdog {
        let _ = w.join();
    }
    if let Some(h) = heartbeat {
        let _ = h.join();
    }

    let killed = killed.load(Ordering::SeqCst);
    let mut envelope = if killed {
        None
    } else if envelope.is_none() && !diagnostic.trim().is_empty() {
        Some(stream::error_envelope(diagnostic.trim()))
    } else {
        envelope
    };
    if let Some(env) = envelope.as_mut().filter(|env| env.is_error) {
        if !diagnostic.trim().is_empty() && !env.result.contains(diagnostic.trim()) {
            env.result.push('\n');
            env.result.push_str(diagnostic.trim());
        }
    }
    state.write_live_status(&format!("iter {n} finished (killed={killed})\n"));
    if !killed {
        match prompt_result {
            Ok(Ok(())) => {}
            Ok(Err(error))
                if error.kind() == std::io::ErrorKind::BrokenPipe
                    && envelope.as_ref().is_some_and(|e| e.is_error) => {}
            Ok(Err(error)) => {
                return Err(format!(
                    "writing iteration prompt to {}: {error}",
                    selection.backend.executable()
                )
                .into())
            }
            Err(_) => return Err("iteration prompt writer panicked".into()),
        }
    }
    Ok(Ran { envelope, killed })
}

/// Construct the exact Claude CLI arguments. Ralph iterations are intentionally
/// fresh, so session persistence is wasted; moving dynamic system sections
/// improves prompt-cache reuse without removing their content.
fn claude_args(cfg: &Config, requested: &str) -> Vec<String> {
    let selection = backend::resolve(cfg, requested);
    let model = selection.model.as_deref().unwrap_or(requested);
    let mut args: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        cfg.output_format.clone(),
    ];
    if cfg.output_format == "stream-json" {
        args.push("--verbose".into());
    }
    if cfg.yolo {
        args.push("--dangerously-skip-permissions".into());
    }
    args.push("--model".into());
    args.push(model.to_string());
    let fallback = backend::resolve(cfg, &cfg.fallback_model);
    if !selection.exclusive && fallback.backend == Backend::Claude {
        if let Some(fb) = fallback.model.filter(|fb| !fb.is_empty() && fb != model) {
            args.extend(["--fallback-model".into(), fb]);
        }
    }
    if !has_extra_flag(&cfg.extra_args, "--no-session-persistence") {
        args.push("--no-session-persistence".into());
    }
    if !has_extra_flag(&cfg.extra_args, "--exclude-dynamic-system-prompt-sections") {
        args.push("--exclude-dynamic-system-prompt-sections".into());
    }
    if extra_effort(&cfg.extra_args).is_none() {
        if let Some(effort) = &selection.effort {
            args.push("--effort".into());
            args.push(effort.clone());
        }
    }
    args.extend(backend::extra_args(cfg, &selection));
    args
}

fn effort_for(cfg: &Config, model: &str) -> Option<String> {
    extra_effort(&cfg.extra_args).or_else(|| backend::resolve(cfg, model).effort)
}

fn extra_effort(args: &[String]) -> Option<String> {
    let mut value = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--effort" {
            value = args.get(index + 1).cloned();
            index += 2;
        } else if let Some(effort) = args[index].strip_prefix("--effort=") {
            value = Some(effort.to_string());
            index += 1;
        } else {
            index += 1;
        }
    }
    value
}

fn has_extra_flag(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|arg| arg == flag || arg.starts_with(&format!("{flag}=")))
}

fn context_warning_key(warning: &str) -> String {
    if let Some((path, _)) = warning.split_once(": oversized progress log") {
        format!("{path}: oversized progress log")
    } else {
        warning.to_string()
    }
}

/// Kill the process group led by `pid` with SIGKILL. The child is spawned as
/// its own group leader (see `run_one`), so the negative-pid target reaps
/// the worker and its subprocesses.
fn kill_group(pid: u32) {
    // A negative PID addresses the worker's process group. Zero would address
    // Ralph's own group, so reject it.
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

/// Minimal PATH lookup for a program (avoids a `which` dependency).
fn which(prog: &str) -> Option<std::path::PathBuf> {
    crate::doctor::executable(prog)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(escalate: u32, abort: u32) -> Config {
        Config {
            escalate_after: escalate,
            abort_after: abort,
            ..Config::default()
        }
    }

    #[test]
    fn iteration_report_head_cost_and_perf() {
        // No envelope → head + cost + summary only.
        let basic = iteration_report(12, 200, 35, None, 0.0431, "did a thing");
        assert!(basic.contains("iter 12/200"), "{basic}");
        assert!(basic.contains("$0.0431"), "{basic}");
        assert!(basic.contains("did a thing"), "{basic}");
        // Unlimited run: pending-work estimate instead of a denominator.
        let unlimited = iteration_report(12, 0, 35, None, 0.5, "more work");
        assert!(unlimited.contains("iter 12 (~35 pending)"), "{unlimited}");
        // With an envelope → tokens, turns, and the api/tools wall-clock split.
        let env = ResultEnvelope {
            duration_ms: 312_000,
            duration_api_ms: 269_000,
            num_turns: 67,
            output_tokens: 20_000,
            cache_read_input_tokens: 3_200_000,
            ..Default::default()
        };
        let rich = iteration_report(64, 0, 5, Some(&env), 1.60, "leaf done");
        assert!(rich.contains("67 turns"), "{rich}");
        assert!(rich.contains("api 269s / tools 43s"), "{rich}");
        assert!(rich.contains("20.0k out"), "{rich}");
        assert!(rich.contains("3.2M cache"), "{rich}");
    }

    #[test]
    fn run_scope_formats_iter_cost_and_duration() {
        assert_eq!(
            run_scope(64, 12.3456, Duration::from_secs(25 * 60 + 7)),
            "iter 64 · $12.35 this run · 25m07s"
        );
        // Past an hour, switch to h/m.
        assert!(run_scope(3, 1.0, Duration::from_secs(3 * 3600 + 5 * 60)).contains("3h05m"));
    }

    #[test]
    fn human_tokens_is_compact() {
        assert_eq!(human_tokens(512), "512");
        assert_eq!(human_tokens(20_000), "20.0k");
        assert_eq!(human_tokens(3_200_000), "3.2M");
    }

    #[test]
    fn backoff_caps() {
        assert_eq!(next_backoff(0, 300, 3600), 300);
        assert_eq!(next_backoff(300, 300, 3600), 600);
        assert_eq!(next_backoff(2000, 300, 3600), 3600); // capped
        assert_eq!(next_backoff(0, 10, 300), 10);
    }

    #[test]
    fn made_resets_streak_and_escalation() {
        let mut t = Thrash::new(&cfg(2, 4));
        assert_eq!(t.record(Verdict::NoProgress, "sonnet"), Action::Continue); // streak 1
        assert_eq!(
            t.record(Verdict::NoProgress, "sonnet"),
            Action::Escalate("opus".into())
        ); // streak 2
        assert_eq!(t.forced_model(), Some("opus".into()));
        assert_eq!(t.record(Verdict::Made, "opus"), Action::Continue);
        assert_eq!(t.forced_model(), None);
        assert_eq!(t.streak, 0);
    }

    #[test]
    fn escalates_up_the_ladder_then_aborts() {
        let mut t = Thrash::new(&cfg(2, 4));
        assert_eq!(t.record(Verdict::NoProgress, "haiku"), Action::Continue); // 1
                                                                              // streak 2 → escalate one tier above the running model (haiku → sonnet)
        assert_eq!(
            t.record(Verdict::NoProgress, "haiku"),
            Action::Escalate("sonnet".into())
        );
        // streak 3 → escalate again (sonnet → opus), computed from forced idx
        assert_eq!(
            t.record(Verdict::NoProgress, "sonnet"),
            Action::Escalate("opus".into())
        );
        // streak 4 → abort
        match t.record(Verdict::NoProgress, "opus") {
            Action::Abort(msg) => assert!(msg.contains("opus")),
            other => panic!("expected abort, got {other:?}"),
        }
    }

    #[test]
    fn excluded_passes_do_not_move_streak() {
        let mut t = Thrash::new(&cfg(2, 4));
        assert_eq!(t.record(Verdict::NoProgress, "sonnet"), Action::Continue); // 1
        assert_eq!(t.record(Verdict::Excluded, "sonnet"), Action::Continue); // still 1
        assert_eq!(t.streak, 1);
        assert_eq!(
            t.record(Verdict::NoProgress, "sonnet"),
            Action::Escalate("opus".into())
        ); // 2
    }

    #[test]
    fn blocked_aborts_without_escalating() {
        let mut t = Thrash::new(&cfg(2, 4));
        // First block: wait one iteration to confirm it wasn't a fluke.
        assert_eq!(t.record(Verdict::Blocked, "sonnet"), Action::Continue);
        // Never escalates the model on a hard block.
        assert_eq!(t.forced_model(), None);
        // Second consecutive block: give up (well before the abort_after=4 budget).
        match t.record(Verdict::Blocked, "sonnet") {
            Action::Abort(msg) => assert!(msg.contains("hard-blocked")),
            other => panic!("expected abort, got {other:?}"),
        }
    }

    #[test]
    fn progress_resets_blocked_streak() {
        let mut t = Thrash::new(&cfg(2, 4));
        assert_eq!(t.record(Verdict::Blocked, "sonnet"), Action::Continue);
        // An intervening productive pass clears the block; it's not "consecutive".
        assert_eq!(t.record(Verdict::Made, "sonnet"), Action::Continue);
        assert_eq!(t.record(Verdict::Blocked, "sonnet"), Action::Continue);
        assert_eq!(t.blocked_streak, 1);
    }

    #[test]
    fn escalation_clamps_at_top() {
        let mut t = Thrash::new(&cfg(1, 9));
        // Already at opus; escalation can't go higher.
        assert_eq!(
            t.record(Verdict::NoProgress, "opus"),
            Action::Escalate("opus".into())
        );
        assert_eq!(
            t.record(Verdict::NoProgress, "opus"),
            Action::Escalate("opus".into())
        );
    }

    #[test]
    fn model_choice_prefers_override_then_decoration_then_default() {
        let cfg = Config::default();
        // An escalation / one-shot override outranks the leaf's decoration.
        let forced = choose_model(&cfg, Some("opus".into()), Some("haiku"));
        assert_eq!(forced.model, "opus");
        assert!(forced.note.is_none());
        // A decoration the ladder carries routes the iteration.
        assert_eq!(choose_model(&cfg, None, Some("haiku")).model, "haiku");
        // No decoration → configured default.
        assert_eq!(choose_model(&cfg, None, None).model, cfg.model);
    }

    #[test]
    fn exclusive_task_survives_escalation_and_concrete_hints_bypass_ladder() {
        let cfg = Config::default();
        assert_eq!(
            choose_model(&cfg, Some("opus".into()), Some("!astra")).model,
            "!astra"
        );
        assert_eq!(choose_model(&cfg, None, Some("fable")).model, "fable");
        let mut thrash = Thrash::new(&cfg);
        for _ in 0..cfg.abort_after - 1 {
            assert_eq!(
                thrash.record(Verdict::NoProgress, "!fable"),
                Action::Continue
            );
        }
        assert!(matches!(
            thrash.record(Verdict::NoProgress, "!fable"),
            Action::Abort(_)
        ));
        assert!(thrash.forced_model().is_none());
    }

    #[test]
    fn decoration_off_the_configured_ladder_is_reported_not_silent() {
        let cfg = Config {
            model: "haiku".into(),
            escalation_ladder: vec!["sonnet".into()],
            ..Config::default()
        };
        let choice = choose_model(&cfg, None, Some("opus"));
        assert_eq!(choice.model, "haiku");
        let note = choice
            .note
            .expect("dropping a declared tier must be logged");
        assert!(note.contains("opus"), "names the declared tier: {note}");
        assert!(note.contains("sonnet"), "names the ladder: {note}");
        assert!(
            note.contains("haiku"),
            "names the model actually used: {note}"
        );
        // An absent or blank decoration is not a drop worth reporting.
        assert!(choose_model(&cfg, None, Some("  ")).note.is_none());
        assert!(choose_model(&cfg, None, None).note.is_none());
    }

    fn arg_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .map(String::as_str)
    }

    #[test]
    fn claude_args_make_fresh_sessions_cache_friendly_and_bound_effort() {
        let cfg = Config::default();
        let args = claude_args(&cfg, "sonnet");
        assert!(args.iter().any(|arg| arg == "--no-session-persistence"));
        assert!(args
            .iter()
            .any(|arg| arg == "--exclude-dynamic-system-prompt-sections"));
        assert_eq!(arg_value(&args, "--effort"), Some("medium"));
        assert_eq!(
            arg_value(&claude_args(&cfg, "haiku"), "--effort"),
            Some("low")
        );
        assert_eq!(
            arg_value(&claude_args(&cfg, "opus"), "--effort"),
            Some("high")
        );
    }

    #[test]
    fn effort_can_be_inherited_or_supplied_by_legacy_extra_args() {
        let inherited = Config {
            effort: "inherit".into(),
            ..Config::default()
        };
        assert_eq!(
            arg_value(&claude_args(&inherited, "sonnet"), "--effort"),
            None
        );

        let legacy = Config {
            extra_args: vec!["--effort".into(), "xhigh".into()],
            ..Config::default()
        };
        let args = claude_args(&legacy, "sonnet");
        assert_eq!(
            args.iter().filter(|arg| arg.as_str() == "--effort").count(),
            1
        );
        assert_eq!(arg_value(&args, "--effort"), Some("xhigh"));
    }

    #[test]
    fn finish_arc_archives_progress_and_resets_counter() {
        use std::fs;
        let repo = std::env::temp_dir().join(format!("ralph-arc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&repo);
        fs::create_dir_all(repo.join(".ralph")).unwrap();
        fs::write(repo.join(".ralph/PROGRESS.md"), "- old arc note\n").unwrap();

        let cfg = Config {
            dir: repo.join(".ralph"),
            progress: repo.join(".ralph/PROGRESS.md"),
            ..Config::default()
        };
        let state = State::open(&cfg.dir).unwrap();
        state.set_iteration(87).unwrap();

        finish_arc(&cfg, &state);

        // Counter reset: a fresh `--max-iterations 30` arc must not halt at boot.
        assert_eq!(state.iteration(), 0);
        // Carry-forward cleared, its content archived.
        assert_eq!(fs::read_to_string(&cfg.progress).unwrap(), "");
        let archived: Vec<_> = fs::read_dir(repo.join(".ralph/archive"))
            .unwrap()
            .map(|e| e.unwrap())
            .filter(|e| e.file_name().to_string_lossy().starts_with("PROGRESS-"))
            .collect();
        assert_eq!(archived.len(), 1);
        assert!(fs::read_to_string(archived[0].path())
            .unwrap()
            .contains("old arc note"));

        // Idempotent-ish: an already-empty PROGRESS archives nothing new.
        finish_arc(&cfg, &state);
        let count = fs::read_dir(repo.join(".ralph/archive")).unwrap().count();
        assert_eq!(count, 1, "empty carry-forward must not archive again");
    }

    #[test]
    fn archive_backlog_moves_the_file() {
        use std::fs;
        use std::path::PathBuf;
        let repo =
            std::env::temp_dir().join(format!("ralph-arch-untracked-{}", std::process::id()));
        let _ = fs::remove_dir_all(&repo);
        fs::create_dir_all(repo.join(".ralph")).unwrap();
        fs::write(repo.join(".ralph/BACKLOG.md"), "items").unwrap();

        let cfg = Config {
            dir: repo.join(".ralph"),
            backlog: repo.join(".ralph/BACKLOG.md"),
            ..Config::default()
        };
        let state = State::open(&cfg.dir).unwrap();
        archive_backlog(&cfg, &state);

        assert!(
            !cfg.backlog.exists(),
            "backlog should be moved even without git"
        );
        let moved: Vec<PathBuf> = fs::read_dir(repo.join(".ralph/archive"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(moved.len(), 1);
        assert!(moved[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("BACKLOG-"));
    }
}
