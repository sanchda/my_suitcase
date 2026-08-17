//! serenity event handler: register the guild slash commands on ready, then on
//! each command interaction enforce the auth gate, resolve the loop from the
//! channel the command arrived in, and dispatch to the ralph bridge. Replies are
//! normal channel messages (a shared audit trail); auth rejections are ephemeral.

use crate::config::{BotConfig, LoopConfig};
use crate::ralph::{Output, Ralph};
use crate::{auth, format, loop_pid, msg};

use serenity::all::{
    ButtonStyle, ChannelId, CommandOptionType, ComponentInteraction, Context, CreateActionRow,
    CreateButton, CreateCommand, CreateCommandOption, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, EditInteractionResponse, EventHandler,
    GuildId, Http, Interaction, Ready,
};
use serenity::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Cadence of the `.ralph/START` trigger poll.
const START_POLL: Duration = Duration::from_secs(3);

/// Shared, thread-safe handle on the loops we spawned, keyed by channel id —
/// shared between the command handlers and each loop's START watcher.
pub type LoopChild = Arc<Mutex<HashMap<u64, Child>>>;

pub struct Handler {
    pub cfg: BotConfig,
    /// The loop processes we spawned this session, kept so we can reap them when
    /// they exit (std Mutex — never held across an `.await`).
    pub loop_child: LoopChild,
    /// Set once we've attempted the opt-in auto-start, so a gateway reconnect
    /// (which re-fires `ready`) never launches a second loop.
    pub autostarted: AtomicBool,
}

/// Spawn the loop and adopt its child handle (into the shared `loop_child`).
/// The caller is responsible for the "already running" check. Returns the new
/// pid. `ralph` writes `loop.pid` itself, so ralphd must not.
pub fn launch_and_record(
    lc: &LoopConfig,
    loop_child: &LoopChild,
    extra: &[String],
) -> Result<u32, String> {
    match Ralph::new(lc).spawn_loop(extra) {
        Ok(child) => {
            let pid = child.id();
            loop_child.lock().unwrap().insert(lc.channel_id, child);
            Ok(pid)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Reap this loop's child if it has exited. Returns the exit status when a reap
/// happened — the START watcher turns an abnormal one into a channel post. A
/// cross-session loop is reparented to init and reaped there, so only this
/// same-session child can zombie (and only it carries a status).
pub fn reap_finished(lc: &LoopConfig, loop_child: &LoopChild) -> Option<ExitStatus> {
    let mut guard = loop_child.lock().unwrap();
    let child = guard.get_mut(&lc.channel_id)?;
    match child.try_wait() {
        Ok(Some(status)) => {
            guard.remove(&lc.channel_id);
            Some(status)
        }
        _ => None,
    }
}

/// The last abort line from `run.log` (timestamp stripped), for the
/// abnormal-exit post.
pub fn last_abort_reason(state_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(state_dir.join("run.log")).ok()?;
    let tail_start = text.len().saturating_sub(16 * 1024);
    let mut start = tail_start;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..]
        .lines()
        .rev()
        .find(|l| l.contains("ABORTED"))
        .map(|l| {
            // Lines look like `HH:MM:SS === ralph ABORTED — reason ===`.
            let l = l.trim();
            let stripped = l.split_once(' ').map(|(_, rest)| rest).unwrap_or(l);
            stripped.trim_matches(|c| c == '=' || c == ' ').to_string()
        })
}

/// The buttons attached to an abnormal-exit post.
fn restart_buttons() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new("ralphd:start")
            .label("Start again")
            .style(ButtonStyle::Primary),
        CreateButton::new("ralphd:start-opus")
            .label("Start on opus")
            .style(ButtonStyle::Secondary),
    ])]
}

/// Background watcher, one per loop: a separate local process (e.g. a claude
/// session running `ralph start`) drops `<state_dir>/START`; when it appears and
/// no loop is running, launch this loop and announce it — no Discord round-trip
/// needed. Runs for the process lifetime.
pub async fn watch_start(lc: LoopConfig, loop_child: LoopChild, http: Arc<Http>) {
    let channel = ChannelId::new(lc.channel_id);
    loop {
        tokio::time::sleep(START_POLL).await;
        let (decision, reaped) = poll_start(&lc, &loop_child);
        // An abnormal exit of the loop WE spawned becomes a post with the
        // reason and restart buttons. (A user-command reap can race this and
        // swallow the status — rare at a 3s poll.) Graceful exits are already
        // announced by ralph's own webhook.
        if let Some(status) = reaped.filter(|s| !s.success()) {
            let reason = last_abort_reason(&lc.state_dir)
                .unwrap_or_else(|| format!("no abort line in run.log ({status})"));
            eprintln!("ralphd[{}]: loop exited abnormally — {reason}", lc.name);
            let _ = channel
                .send_message(
                    &http,
                    CreateMessage::new()
                        .content(format!("🔴 **loop exited** — {reason}"))
                        .components(restart_buttons()),
                )
                .await;
        }
        match decision {
            StartDecision::NoTrigger => {}
            StartDecision::AlreadyRunning(pid) => {
                eprintln!(
                    "ralphd[{}]: START ignored — loop already running (pid {pid})",
                    lc.name
                );
            }
            StartDecision::Launch => match launch_and_record(&lc, &loop_child, &[]) {
                Ok(pid) => {
                    eprintln!("ralphd[{}]: START trigger → launched ralph (pid {pid})", lc.name);
                    let _ = channel
                        .say(&http, format!("🟢 started ralph (pid {pid}) — via `ralph start`"))
                        .await;
                }
                Err(e) => {
                    eprintln!("ralphd[{}]: START trigger failed: {e}", lc.name);
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
/// and decide whether to launch. Returns the reaped exit status (if this poll
/// reaped one) alongside the decision. Factored out of [`watch_start`] so the
/// trigger logic is testable without a gateway or a real loop.
fn poll_start(lc: &LoopConfig, loop_child: &LoopChild) -> (StartDecision, Option<ExitStatus>) {
    let reaped = reap_finished(lc, loop_child);
    let marker = lc.state_dir.join("START");
    if !marker.exists() {
        return (StartDecision::NoTrigger, reaped);
    }
    // Consume the trigger regardless of outcome so it fires once.
    let _ = std::fs::remove_file(&marker);
    let decision = match loop_pid::running(&lc.state_dir) {
        Some(pid) => StartDecision::AlreadyRunning(pid),
        None => StartDecision::Launch,
    };
    (decision, reaped)
}

/// One shelled-out mutation's reply: stdout on success, stderr on rejection.
fn shell_reply(what: &str, out: std::io::Result<Output>) -> String {
    match out {
        Ok(o) if o.ok => {
            let s = o.stdout.trim();
            if s.is_empty() {
                format!("{what} ok")
            } else {
                s.to_string()
            }
        }
        Ok(o) => format!("rejected: {}", o.stderr.trim()),
        Err(e) => format!("{what} failed: {e}"),
    }
}

impl Handler {
    /// Opt-in auto-start: launch every loop configured for it on connect, unless
    /// one is already running, announcing each outcome in its channel. Guarded by
    /// `autostarted` so a reconnect can't spawn duplicates.
    async fn autostart(&self, ctx: &Context) {
        if self.autostarted.swap(true, Ordering::SeqCst) {
            return; // a prior `ready` already handled it this process
        }
        for lc in self.cfg.loops.values().filter(|l| l.autostart) {
            if let Some(pid) = loop_pid::running(&lc.state_dir) {
                eprintln!(
                    "ralphd[{}]: autostart skipped — loop already running (pid {pid})",
                    lc.name
                );
                continue;
            }
            let channel = ChannelId::new(lc.channel_id);
            match launch_and_record(lc, &self.loop_child, &[]) {
                Ok(pid) => {
                    eprintln!("ralphd[{}]: auto-started ralph (pid {pid})", lc.name);
                    let _ = channel
                        .say(&ctx.http, format!("🟢 auto-started ralph (pid {pid})"))
                        .await;
                }
                Err(e) => {
                    eprintln!("ralphd[{}]: auto-start failed: {e}", lc.name);
                    let _ = channel
                        .say(&ctx.http, format!("⚠️ ralph auto-start failed: {e}"))
                        .await;
                }
            }
        }
    }

    /// A button click from an abnormal-exit post: same auth gate as commands,
    /// then start that channel's loop (optionally on opus).
    async fn handle_component(&self, ctx: &Context, comp: ComponentInteraction) {
        if !auth::authorized(comp.channel_id.get(), comp.user.id.get(), &self.cfg) {
            let deny = CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content("not authorized in this channel")
                    .ephemeral(true),
            );
            let _ = comp.create_response(&ctx.http, deny).await;
            return;
        }
        let reply = match comp.data.custom_id.as_str() {
            "ralphd:start" => self.component_start(comp.channel_id.get(), &[]),
            "ralphd:start-opus" => {
                self.component_start(comp.channel_id.get(), &["--model".into(), "opus".into()])
            }
            other => format!("unknown button `{other}`"),
        };
        let _ = comp
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new().content(reply),
                ),
            )
            .await;
    }

    fn component_start(&self, channel_id: u64, extra: &[String]) -> String {
        let Some(lc) = self.cfg.loops.get(&channel_id) else {
            return "no loop configured for this channel".into();
        };
        reap_finished(lc, &self.loop_child);
        if let Some(pid) = loop_pid::running(&lc.state_dir) {
            return format!("already running (pid {pid})");
        }
        match launch_and_record(lc, &self.loop_child, extra) {
            Ok(pid) if extra.is_empty() => format!("started ralph (pid {pid})"),
            Ok(pid) => format!("started ralph (pid {pid}) — {}", extra.join(" ")),
            Err(e) => format!("failed to start: {e}"),
        }
    }

    /// Turn a command name plus its option resolvers into the reply string, for
    /// the loop that owns `channel_id`. `opt`/`flag` resolve options by name,
    /// keeping the serenity plumbing in `interaction_create`.
    async fn dispatch(
        &self,
        channel_id: u64,
        name: &str,
        opt: impl Fn(&str) -> Option<String>,
        flag: impl Fn(&str) -> bool,
    ) -> String {
        let Some(lc) = self.cfg.loops.get(&channel_id) else {
            return "no loop configured for this channel".into();
        };
        reap_finished(lc, &self.loop_child);
        let r = Ralph::new(lc);
        // Trimmed-empty options read as absent, so an option left blank in the
        // client never becomes a literal empty argument.
        let some = |n: &str| opt(n).filter(|v| !v.trim().is_empty());
        match name {
            "start" => {
                if let Some(pid) = loop_pid::running(&lc.state_dir) {
                    return format!("already running (pid {pid})");
                }
                // An optional model overrides the launch profile's default for
                // this run (appended, so ralph's last-wins parsing picks it up).
                let extra = match some("model") {
                    Some(m) => vec!["--model".into(), m],
                    None => Vec::new(),
                };
                match launch_and_record(lc, &self.loop_child, &extra) {
                    Ok(pid) => format!("started ralph (pid {pid})"),
                    Err(e) => format!("failed to start: {e}"),
                }
            }
            "stop" => {
                let now = flag("now");
                match r.stop(now).await {
                    Ok(o) if o.ok && now => "stopping now — the loop was signalled".into(),
                    Ok(o) if o.ok => "stop requested — halts after the current iteration".into(),
                    Ok(o) => format!("stop failed: {}", o.stderr.trim()),
                    Err(e) => format!("stop failed: {e}"),
                }
            }
            // ralph owns the ladder and canonicalizes the tier, so echo its word.
            "model" => {
                let tier = some("tier").unwrap_or_default();
                shell_reply("model", r.model(&tier).await)
            }
            "status" | "next" => match r.status_json().await {
                Ok(o) if o.ok => {
                    let running = loop_pid::running(&lc.state_dir).is_some();
                    format::status_message(&o.stdout, running)
                }
                Ok(o) => format!("status failed: {}", o.stderr.trim()),
                Err(e) => format!("status failed: {e}"),
            },
            "add" => {
                let title = some("title").unwrap_or_default();
                let out = r
                    .add(
                        some("id").as_deref(),
                        &title,
                        some("verify").as_deref(),
                        some("under").as_deref(),
                    )
                    .await;
                shell_reply("add", out)
            }
            "drop" => {
                let id = some("id").unwrap_or_default();
                shell_reply("drop", r.drop_task(&id, flag("recursive")).await)
            }
            "uncheck" => {
                let id = some("id").unwrap_or_default();
                shell_reply("uncheck", r.uncheck(&id).await)
            }
            "done" => {
                let id = some("id").unwrap_or_default();
                shell_reply("done", r.done(&id).await)
            }
            "backlog-edit" => {
                let id = some("id").unwrap_or_default();
                let title = some("title").unwrap_or_default();
                let verify = some("verify").unwrap_or_default();
                shell_reply("backlog edit", r.backlog_edit(&id, &title, &verify).await)
            }
            other => format!("unknown command `{other}`"),
        }
    }
}

/// The guild slash commands, in registration order.
fn commands() -> Vec<CreateCommand> {
    let req_str = |name: &str, desc: &str| {
        CreateCommandOption::new(CommandOptionType::String, name, desc).required(true)
    };
    let opt_str = |name: &str, desc: &str| {
        CreateCommandOption::new(CommandOptionType::String, name, desc).required(false)
    };
    let opt_bool = |name: &str, desc: &str| {
        CreateCommandOption::new(CommandOptionType::Boolean, name, desc).required(false)
    };
    vec![
        CreateCommand::new("start")
            .description("Start the ralph loop")
            .add_option(opt_str("model", "model override for this run")),
        CreateCommand::new("stop")
            .description("Stop after the current iteration, or immediately with now")
            .add_option(opt_bool("now", "signal the running loop instead of waiting")),
        CreateCommand::new("model")
            .description("One-shot model override for the next iteration")
            .add_option(req_str("tier", "a tier on the escalation ladder")),
        CreateCommand::new("status")
            .description("Loop status: iteration, pending count, current + next tasks"),
        CreateCommand::new("next").description("Show the current and upcoming backlog tasks"),
        CreateCommand::new("add")
            .description("Queue a backlog task (validated before saving)")
            .add_option(req_str("title", "task title"))
            .add_option(opt_str("verify", "how to verify the task is done"))
            .add_option(opt_str("id", "explicit id, e.g. 3.1.1 (inserted under 3.1)"))
            .add_option(opt_str("under", "parent id — auto-numbers the next child")),
        CreateCommand::new("drop")
            .description("Remove a backlog task (archived, never destroyed)")
            .add_option(req_str("id", "backlog task id"))
            .add_option(opt_bool("recursive", "also drop its children")),
        CreateCommand::new("uncheck")
            .description("Reopen a checked-off backlog task")
            .add_option(req_str("id", "backlog task id")),
        CreateCommand::new("done")
            .description("Check off a backlog task")
            .add_option(req_str("id", "backlog task id")),
        CreateCommand::new("backlog-edit")
            .description("Edit a backlog task's title and verify (validated before saving)")
            .add_option(req_str("id", "backlog task id"))
            .add_option(req_str("title", "new task title"))
            .add_option(req_str("verify", "new verify criteria")),
        CreateCommand::new("msg")
            .description("Steer the loop through its persistent claude session")
            .add_option(req_str("message", "what to tell the session"))
            .add_option(opt_bool("new", "start a fresh session, archiving the old one")),
    ]
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        let guild = GuildId::new(self.cfg.guild_id);
        // set_commands REPLACES the guild's whole command set, so a second ralphd
        // in this guild silently unregisters ours (and we unregister its). Name
        // what we are overwriting so that shows up in the log rather than as
        // commands mysteriously vanishing.
        if let Ok(existing) = guild.get_commands(&ctx.http).await {
            let names: Vec<&str> = existing.iter().map(|c| c.name.as_str()).collect();
            eprintln!(
                "ralphd: replacing ALL {} guild commands in {} ({}) — run exactly one ralphd per guild",
                names.len(),
                self.cfg.guild_id,
                names.join(", ")
            );
        }
        match guild.set_commands(&ctx.http, commands()).await {
            Ok(cmds) => eprintln!(
                "ralphd: ready as {} — registered {} guild commands in {} for {} loop(s)",
                ready.user.name,
                cmds.len(),
                self.cfg.guild_id,
                self.cfg.loops.len(),
            ),
            Err(e) => eprintln!("ralphd: failed to register guild commands: {e}"),
        }
        self.autostart(&ctx).await;
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let command = match interaction {
            Interaction::Command(command) => command,
            Interaction::Component(comp) => {
                self.handle_component(&ctx, comp).await;
                return;
            }
            _ => return,
        };

        let channel_id = command.channel_id.get();
        let user_id = command.user.id.get();

        // Auth gate: refuse anything outside the one user and a channel some
        // loop claims, with an ephemeral notice, and take no further action.
        if !auth::authorized(channel_id, user_id, &self.cfg) {
            let deny = CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content("not authorized in this channel")
                    .ephemeral(true),
            );
            let _ = command.create_response(&ctx.http, deny).await;
            return;
        }

        let value = |name: &str| command.data.options.iter().find(|o| o.name == name);
        let get = |name: &str| -> Option<String> {
            value(name).and_then(|o| o.value.as_str()).map(String::from)
        };
        let get_bool =
            |name: &str| -> bool { value(name).and_then(|o| o.value.as_bool()).unwrap_or(false) };

        // `/msg` drives a claude session, which far exceeds Discord's 3s ack
        // window: defer first, then stream the session, keeping one live status
        // message current with token usage and finishing with the cost.
        if command.data.name == "msg" {
            let text = get("message").unwrap_or_default();
            if text.trim().is_empty() {
                return;
            }
            let Some(lc) = self.cfg.loops.get(&channel_id) else {
                return;
            };
            if command.defer(&ctx.http).await.is_err() {
                return;
            }
            match Ralph::new(lc).spawn_msg(&text, get_bool("new")) {
                Ok(child) => msg::drive(&ctx, &command, child).await,
                Err(e) => {
                    let edit = EditInteractionResponse::new()
                        .content(format!("could not start the session: {e}"));
                    let _ = command.edit_response(&ctx.http, edit).await;
                }
            }
            return;
        }

        let reply = self
            .dispatch(channel_id, &command.data.name, get, get_bool)
            .await;
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

    fn tmp_loop() -> LoopConfig {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ralphd-start-{}-{}",
            std::process::id(),
            N.fetch_add(1, O::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        LoopConfig {
            name: "test".into(),
            channel_id: 2,
            working_dir: dir.clone(),
            ralph_config: dir.join("ralph.toml"),
            state_dir: dir,
            ralph_args: vec![],
            webhook: None,
            autostart: false,
        }
    }

    fn no_children() -> LoopChild {
        Arc::new(Mutex::new(HashMap::new()))
    }

    #[test]
    fn poll_start_no_marker_is_no_trigger() {
        let lc = tmp_loop();
        assert_eq!(
            poll_start(&lc, &no_children()),
            (StartDecision::NoTrigger, None)
        );
    }

    #[test]
    fn poll_start_launches_and_consumes_marker() {
        let lc = tmp_loop();
        std::fs::write(lc.state_dir.join("START"), "go").unwrap();
        assert_eq!(
            poll_start(&lc, &no_children()),
            (StartDecision::Launch, None)
        );
        assert!(!lc.state_dir.join("START").exists(), "marker must be consumed");
    }

    #[test]
    fn poll_start_skips_when_a_loop_is_running() {
        let lc = tmp_loop();
        std::fs::write(lc.state_dir.join("START"), "go").unwrap();
        // A live pid (our own) recorded in the pidfile reads as "running".
        loop_pid::write(&lc.state_dir, std::process::id()).unwrap();
        assert_eq!(
            poll_start(&lc, &no_children()),
            (StartDecision::AlreadyRunning(std::process::id()), None)
        );
        assert!(
            !lc.state_dir.join("START").exists(),
            "marker consumed even when skipped"
        );
    }

    #[test]
    fn reap_returns_the_childs_exit_status_and_leaves_the_pidfile_to_ralph() {
        let lc = tmp_loop();
        // A real short-lived child with a nonzero exit.
        let child = std::process::Command::new("sh")
            .args(["-c", "exit 3"])
            .spawn()
            .unwrap();
        loop_pid::write(&lc.state_dir, child.id()).unwrap();
        let lchild: LoopChild = Arc::new(Mutex::new(HashMap::from([(lc.channel_id, child)])));
        let status = loop {
            if let Some(s) = reap_finished(&lc, &lchild) {
                break s;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(status.code(), Some(3));
        assert!(
            !lchild.lock().unwrap().contains_key(&lc.channel_id),
            "child handle cleared"
        );
        assert!(
            loop_pid::read(&lc.state_dir).is_some(),
            "ralph owns loop.pid — ralphd must not delete it"
        );
        // Nothing left to reap.
        assert_eq!(reap_finished(&lc, &lchild), None);
    }

    #[test]
    fn reaping_one_loop_leaves_the_others_children_alone() {
        let a = tmp_loop();
        let mut b = tmp_loop();
        b.channel_id = 99;
        let quick = std::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let slow = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let slow_pid = slow.id();
        let lchild: LoopChild = Arc::new(Mutex::new(HashMap::from([
            (a.channel_id, quick),
            (b.channel_id, slow),
        ])));
        loop {
            if reap_finished(&a, &lchild).is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(reap_finished(&b, &lchild), None, "the other loop still runs");
        let mut survivor = lchild
            .lock()
            .unwrap()
            .remove(&b.channel_id)
            .expect("other child retained");
        assert_eq!(survivor.id(), slow_pid);
        let _ = survivor.kill();
        let _ = survivor.wait();
    }

    #[test]
    fn abort_reason_is_last_aborted_line_without_timestamp() {
        let lc = tmp_loop();
        std::fs::write(
            lc.state_dir.join("run.log"),
            "10:00:01 iter 3 → sonnet\n\
             10:05:00 === ralph ABORTED — no progress after 4 iterations (escalated to opus) ===\n\
             10:05:01 tail noise\n",
        )
        .unwrap();
        let reason = last_abort_reason(&lc.state_dir).unwrap();
        assert_eq!(
            reason,
            "ralph ABORTED — no progress after 4 iterations (escalated to opus)"
        );
        // Absent file → None (caller falls back to the raw exit status).
        let empty = tmp_loop();
        assert_eq!(last_abort_reason(&empty.state_dir), None);
    }
}
