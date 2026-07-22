//! serenity event handler: register the guild slash commands on ready, then on
//! each command interaction enforce the single-tenant auth gate and dispatch to
//! the ralph bridge. Replies are normal channel messages (a shared audit trail);
//! auth rejections are ephemeral.

use crate::config::BotConfig;
use crate::ralph::Ralph;
use crate::{auth, btw, format, loop_pid, model};

use serenity::all::{
    CommandOptionType, Context, CreateCommand, CreateCommandOption, CreateInteractionResponse,
    CreateInteractionResponseMessage, EditInteractionResponse, EventHandler, GuildId, Interaction,
    Ready,
};
use serenity::async_trait;

pub struct Handler {
    pub cfg: BotConfig,
    /// The loop process we spawned this session, kept so we can reap it when it
    /// exits (std Mutex — never held across an `.await`).
    pub loop_child: std::sync::Mutex<Option<std::process::Child>>,
}

impl Handler {
    fn ralph(&self) -> Ralph {
        Ralph::new(&self.cfg)
    }

    /// Reap the loop we spawned this session if it has exited, clearing its
    /// pidfile so /start works again. A cross-session loop is reparented to init
    /// and reaped there, so only this same-session child can zombie.
    fn reap_finished_loop(&self) {
        let mut guard = self.loop_child.lock().unwrap();
        if let Some(child) = guard.as_mut() {
            if let Ok(Some(_status)) = child.try_wait() {
                *guard = None;
                loop_pid::clear(&self.cfg.state_dir);
            }
        }
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
                match r.spawn_loop(&extra) {
                    Ok(child) => {
                        let pid = child.id();
                        let _ = loop_pid::write(&self.cfg.state_dir, pid);
                        *self.loop_child.lock().unwrap() = Some(child);
                        format!("started ralph (pid {pid})")
                    }
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
