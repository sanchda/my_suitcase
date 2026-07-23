//! serenity event handler: register the guild slash commands on ready, then on
//! each command interaction enforce the single-tenant auth gate and dispatch to
//! the ralph bridge. Replies are normal channel messages (a shared audit trail);
//! auth rejections are ephemeral.

use crate::config::BotConfig;
use crate::ralph::Ralph;
use crate::{auth, btw, format, loop_pid, model};

use serenity::all::{
    ChannelId, CommandOptionType, Context, CreateCommand, CreateCommandOption,
    CreateInteractionResponse, CreateInteractionResponseMessage, EditInteractionResponse,
    EventHandler, GuildId, Http, Interaction, Ready,
};
use serenity::async_trait;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Cadence of the `.ralph/START` trigger poll.
const START_POLL: Duration = Duration::from_secs(3);

/// Shared, thread-safe handle on the loop we spawned — shared between the command
/// handlers and the START watcher so both track the one loop.
pub type LoopChild = Arc<Mutex<Option<Child>>>;

pub struct Handler {
    pub cfg: BotConfig,
    /// The loop process we spawned this session, kept so we can reap it when it
    /// exits (std Mutex — never held across an `.await`).
    pub loop_child: LoopChild,
    /// Set once we've attempted the opt-in auto-start, so a gateway reconnect
    /// (which re-fires `ready`) never launches a second loop.
    pub autostarted: AtomicBool,
}

/// Spawn the loop, record its pid, and adopt its child handle (into the shared
/// `loop_child`). The caller is responsible for the "already running" check.
/// Returns the new pid. Used by `/start`, auto-start, and the START watcher.
pub fn launch_and_record(cfg: &BotConfig, loop_child: &LoopChild, extra: &[String]) -> Result<u32, String> {
    match Ralph::new(cfg).spawn_loop(extra) {
        Ok(child) => {
            let pid = child.id();
            let _ = loop_pid::write(&cfg.state_dir, pid);
            *loop_child.lock().unwrap() = Some(child);
            Ok(pid)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Reap the loop we spawned if it has exited, clearing its pidfile so a new start
/// works again. A cross-session loop is reparented to init and reaped there, so
/// only this same-session child can zombie.
pub fn reap_and_clear(cfg: &BotConfig, loop_child: &LoopChild) {
    let mut guard = loop_child.lock().unwrap();
    if let Some(child) = guard.as_mut() {
        if let Ok(Some(_status)) = child.try_wait() {
            *guard = None;
            loop_pid::clear(&cfg.state_dir);
        }
    }
}

/// Background watcher: a separate local process (e.g. a claude session running
/// `ralph start`) drops `<state_dir>/START`; when it appears and no loop is
/// running, launch the tracked loop and announce it — no Discord round-trip
/// needed to start the ralphd-managed loop. Runs for the process lifetime.
pub async fn watch_start(cfg: BotConfig, loop_child: LoopChild, http: Arc<Http>) {
    let channel = ChannelId::new(cfg.channel_id);
    loop {
        tokio::time::sleep(START_POLL).await;
        match poll_start(&cfg, &loop_child) {
            StartDecision::NoTrigger => {}
            StartDecision::AlreadyRunning(pid) => {
                eprintln!("ralphd: START ignored — loop already running (pid {pid})");
            }
            StartDecision::Launch => match launch_and_record(&cfg, &loop_child, &[]) {
                Ok(pid) => {
                    eprintln!("ralphd: START trigger → launched ralph (pid {pid})");
                    let _ = channel
                        .say(&http, format!("🟢 started ralph (pid {pid}) — via `ralph start`"))
                        .await;
                }
                Err(e) => {
                    eprintln!("ralphd: START trigger failed: {e}");
                    let _ = channel
                        .say(&http, format!("⚠️ `ralph start` trigger failed: {e}"))
                        .await;
                }
            },
        }
    }
}

/// What one START poll should do (pure decision — no launch, no Discord).
#[derive(Debug, PartialEq, Eq)]
enum StartDecision {
    NoTrigger,
    AlreadyRunning(u32),
    Launch,
}

/// Reap a finished loop, then inspect the `START` marker: consume it if present
/// and decide whether to launch. Factored out of [`watch_start`] so the trigger
/// logic is testable without a gateway or a real loop.
fn poll_start(cfg: &BotConfig, loop_child: &LoopChild) -> StartDecision {
    // Keep the pidfile honest so a finished loop can be relaunched.
    reap_and_clear(cfg, loop_child);
    let marker = cfg.state_dir.join("START");
    if !marker.exists() {
        return StartDecision::NoTrigger;
    }
    // Consume the trigger regardless of outcome so it fires once.
    let _ = std::fs::remove_file(&marker);
    match loop_pid::running(&cfg.state_dir) {
        Some(pid) => StartDecision::AlreadyRunning(pid),
        None => StartDecision::Launch,
    }
}

impl Handler {
    fn ralph(&self) -> Ralph {
        Ralph::new(&self.cfg)
    }

    /// Spawn the loop, record its pid, and adopt its child handle. The caller is
    /// responsible for the "already running" check. Returns the new pid.
    fn launch_loop(&self, extra: &[String]) -> Result<u32, String> {
        launch_and_record(&self.cfg, &self.loop_child, extra)
    }

    /// Opt-in auto-start: launch the loop on connect unless one is already
    /// running, announcing the outcome in the channel. Guarded by `autostarted`
    /// so a reconnect can't spawn a duplicate.
    async fn autostart(&self, ctx: &Context) {
        if self.autostarted.swap(true, Ordering::SeqCst) {
            return; // a prior `ready` already handled it this process
        }
        if let Some(pid) = loop_pid::running(&self.cfg.state_dir) {
            eprintln!("ralphd: autostart skipped — loop already running (pid {pid})");
            return;
        }
        let channel = ChannelId::new(self.cfg.channel_id);
        match self.launch_loop(&[]) {
            Ok(pid) => {
                eprintln!("ralphd: auto-started ralph (pid {pid})");
                let _ = channel
                    .say(&ctx.http, format!("🟢 auto-started ralph (pid {pid})"))
                    .await;
            }
            Err(e) => {
                eprintln!("ralphd: auto-start failed: {e}");
                let _ = channel
                    .say(&ctx.http, format!("⚠️ ralph auto-start failed: {e}"))
                    .await;
            }
        }
    }

    /// Reap the loop we spawned this session if it has exited, clearing its
    /// pidfile so a new start works again.
    fn reap_finished_loop(&self) {
        reap_and_clear(&self.cfg, &self.loop_child);
    }

    /// Turn a command name plus an option resolver into the reply string. `opt`
    /// resolves a string option by name, keeping the serenity plumbing in
    /// `interaction_create`.
    fn dispatch(&self, name: &str, opt: impl Fn(&str) -> Option<String>) -> String {
        self.reap_finished_loop();
        let r = self.ralph();
        match name {
            "start" => {
                if let Some(pid) = loop_pid::running(&self.cfg.state_dir) {
                    return format!("already running (pid {pid})");
                }
                // An optional model overrides the launch profile's default for
                // this run (appended, so ralph's last-wins parsing picks it up).
                let extra = match opt("model") {
                    Some(m) if !m.trim().is_empty() => vec!["--model".into(), m],
                    _ => Vec::new(),
                };
                match self.launch_loop(&extra) {
                    Ok(pid) => format!("started ralph (pid {pid})"),
                    Err(e) => format!("failed to start: {e}"),
                }
            }
            "stop" => match r.stop() {
                Ok(o) if o.ok => "stop requested — halts after the current iteration".into(),
                Ok(o) => format!("stop failed: {}", o.stderr.trim()),
                Err(e) => format!("stop failed: {e}"),
            },
            "model" => {
                let raw = opt("tier").unwrap_or_default();
                match model::validate_tier(&raw) {
                    Some(tier) => match r.write_model(tier) {
                        Ok(()) => format!("next iteration → {tier} (one-shot)"),
                        Err(e) => format!("could not write MODEL: {e}"),
                    },
                    None => format!("unknown tier `{raw}` (use haiku, sonnet, or opus)"),
                }
            }
            "status" | "next" => match r.status_json() {
                Ok(o) if o.ok => {
                    let running = loop_pid::running(&self.cfg.state_dir).is_some();
                    format::status_message(&o.stdout, running)
                }
                Ok(o) => format!("status failed: {}", o.stderr.trim()),
                Err(e) => format!("status failed: {e}"),
            },
            "backlog-add" => {
                let title = opt("title").unwrap_or_default();
                let verify = opt("verify").unwrap_or_default();
                match r.backlog_add(&title, &verify) {
                    Ok(o) if o.ok => o.stdout.trim().to_string(),
                    Ok(o) => format!("rejected: {}", o.stderr.trim()),
                    Err(e) => format!("backlog add failed: {e}"),
                }
            }
            "backlog-edit" => {
                let id = opt("id").unwrap_or_default();
                let title = opt("title").unwrap_or_default();
                let verify = opt("verify").unwrap_or_default();
                match r.backlog_edit(&id, &title, &verify) {
                    Ok(o) if o.ok => o.stdout.trim().to_string(),
                    Ok(o) => format!("rejected: {}", o.stderr.trim()),
                    Err(e) => format!("backlog edit failed: {e}"),
                }
            }
            other => format!("unknown command `{other}`"),
        }
    }
}

/// The seven guild slash commands, in registration order.
fn commands() -> Vec<CreateCommand> {
    let req_str = |name: &str, desc: &str| {
        CreateCommandOption::new(CommandOptionType::String, name, desc).required(true)
    };
    let opt_str = |name: &str, desc: &str| {
        CreateCommandOption::new(CommandOptionType::String, name, desc).required(false)
    };
    vec![
        CreateCommand::new("start")
            .description("Start the ralph loop")
            .add_option(opt_str("model", "model override for this run")),
        CreateCommand::new("stop").description("Gracefully stop after the current iteration"),
        CreateCommand::new("model")
            .description("One-shot model override for the next iteration")
            .add_option(req_str("tier", "haiku, sonnet, or opus")),
        CreateCommand::new("status")
            .description("Loop status: iteration, pending count, current + next tasks"),
        CreateCommand::new("next").description("Show the current and upcoming backlog tasks"),
        CreateCommand::new("backlog-add")
            .description("Append a backlog task (validated before saving)")
            .add_option(req_str("title", "task title"))
            .add_option(req_str("verify", "how to verify the task is done")),
        CreateCommand::new("backlog-edit")
            .description("Edit a backlog task's title and verify (validated before saving)")
            .add_option(req_str("id", "backlog task id"))
            .add_option(req_str("title", "new task title"))
            .add_option(req_str("verify", "new verify criteria")),
        CreateCommand::new("btw")
            .description("Run a one-off yolo claude session with your message")
            .add_option(req_str("message", "what to tell claude"))
            .add_option(opt_str("model", "model override for this session")),
    ]
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        let guild = GuildId::new(self.cfg.guild_id);
        match guild.set_commands(&ctx.http, commands()).await {
            Ok(cmds) => eprintln!(
                "ralphd: ready as {} — registered {} guild commands in {}",
                ready.user.name,
                cmds.len(),
                self.cfg.guild_id
            ),
            Err(e) => eprintln!("ralphd: failed to register guild commands: {e}"),
        }
        if self.cfg.autostart {
            self.autostart(&ctx).await;
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Interaction::Command(command) = interaction else {
            return;
        };

        let channel_id = command.channel_id.get();
        let user_id = command.user.id.get();

        // Auth gate: refuse anything outside the single configured channel+user
        // with an ephemeral notice and take no further action.
        if !auth::authorized(channel_id, user_id, &self.cfg) {
            let deny = CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content("not authorized in this channel")
                    .ephemeral(true),
            );
            let _ = command.create_response(&ctx.http, deny).await;
            return;
        }

        let get = |name: &str| -> Option<String> {
            command
                .data
                .options
                .iter()
                .find(|o| o.name == name)
                .and_then(|o| o.value.as_str())
                .map(String::from)
        };

        // `/btw` runs a full claude session, which far exceeds Discord's 3s ack
        // window: defer first, then stream the session, keeping one live status
        // message current with token usage and finishing with the cost.
        if command.data.name == "btw" {
            let message = get("message").unwrap_or_default();
            let model = get("model");
            if message.trim().is_empty() {
                return;
            }
            if command.defer(&ctx.http).await.is_err() {
                return;
            }
            match self.ralph().spawn_btw(&message, model.as_deref()) {
                Ok(child) => btw::drive(&ctx, &command, child).await,
                Err(e) => {
                    let edit = EditInteractionResponse::new()
                        .content(format!("could not start claude: {e}"));
                    let _ = command.edit_response(&ctx.http, edit).await;
                }
            }
            return;
        }

        let reply = self.dispatch(&command.data.name, get);
        let response = CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new().content(reply),
        );
        if let Err(e) = command.create_response(&ctx.http, response).await {
            eprintln!("ralphd: failed to send response: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as O};

    fn tmp_cfg() -> BotConfig {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ralphd-start-{}-{}",
            std::process::id(),
            N.fetch_add(1, O::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        BotConfig {
            token: "t".into(),
            guild_id: 1,
            channel_id: 2,
            user_id: 3,
            working_dir: dir.clone(),
            state_dir: dir,
            ralph_args: vec![],
            autostart: false,
        }
    }

    #[test]
    fn poll_start_no_marker_is_no_trigger() {
        let cfg = tmp_cfg();
        let lc: LoopChild = Arc::new(Mutex::new(None));
        assert_eq!(poll_start(&cfg, &lc), StartDecision::NoTrigger);
    }

    #[test]
    fn poll_start_launches_and_consumes_marker() {
        let cfg = tmp_cfg();
        let lc: LoopChild = Arc::new(Mutex::new(None));
        std::fs::write(cfg.state_dir.join("START"), "go").unwrap();
        assert_eq!(poll_start(&cfg, &lc), StartDecision::Launch);
        assert!(!cfg.state_dir.join("START").exists(), "marker must be consumed");
    }

    #[test]
    fn poll_start_skips_when_a_loop_is_running() {
        let cfg = tmp_cfg();
        let lc: LoopChild = Arc::new(Mutex::new(None));
        std::fs::write(cfg.state_dir.join("START"), "go").unwrap();
        // A live pid (our own) recorded in the pidfile reads as "running".
        loop_pid::write(&cfg.state_dir, std::process::id()).unwrap();
        assert_eq!(
            poll_start(&cfg, &lc),
            StartDecision::AlreadyRunning(std::process::id())
        );
        assert!(
            !cfg.state_dir.join("START").exists(),
            "marker consumed even when skipped"
        );
    }
}
