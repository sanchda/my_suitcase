mod auth;
mod card;
mod chunk;
mod config;
mod format;
mod handler;
mod ledger;
mod loop_pid;
mod msg;
mod ralph;

use serenity::prelude::*;

const USAGE: &str = "\
ralphd — Discord control bridge for ralph loops, one channel per loop

Usage:
  DISCORD_BOT_TOKEN=<token> ralphd [--config <path>]
  DISCORD_BOT_TOKEN=<token> ralphd [options] -- [ralph args...]

The first form drives many loops from a TOML file (default
~/.config/ralphd.toml); the second is the single-loop shorthand.

  guild = 123
  user  = 456

  [[loop]]
  name      = \"number-grove\"
  channel   = 111
  dir       = \"/home/me/dev/number_grove\"
  args      = [\"--model\", \"sonnet\"]
  webhook   = \"https://discord.com/api/webhooks/…\"   # optional, per loop
  autostart = false                                   # optional

Options (each also settable via the environment):
  --config <path>       Multi-loop TOML config           [env RALPHD_CONFIG]
  --guild <id>          Discord server (guild) id        [env RALPHD_GUILD_ID]
  --channel <id>        Channel commands are accepted in [env RALPHD_CHANNEL_ID]
  --user <id>           The one authorized user id       [env RALPHD_USER_ID]
  --working-dir <path>  Repo the loop runs in (default: .) [env RALPHD_WORKING_DIR]
  --autostart           Start the loop on connect        [env RALPHD_AUTOSTART]
  -h, --help            Show this help

Required environment:
  DISCORD_BOT_TOKEN     Bot token (env only, never a flag)

Everything after `--` is forwarded verbatim to `ralph` when you run /start.
Slash commands: /start /stop /model /status /next /add /drop /uncheck /done
/backlog-edit /msg — each acts on the loop that owns the channel you type it in.

While a loop runs, ralphd keeps one pinned status card per channel edited in
place; when a loop it spawned dies abnormally it posts the abort reason with
Start-again / Start-on-opus buttons. Pinning needs the Manage Messages
permission. Registration is guild-wide and replaces the whole command set, so
run exactly ONE ralphd per guild.
";

#[tokio::main]
async fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    // Only look for help before `--`; anything after it belongs to ralph.
    let pre = &argv[..argv.iter().position(|a| a == "--").unwrap_or(argv.len())];
    if pre.first().map(String::as_str) == Some("help")
        || pre.iter().any(|a| a == "-h" || a == "--help")
    {
        print!("{USAGE}");
        return;
    }

    let cfg = match config::parse(&argv, |k| std::env::var(k).ok()) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("ralphd: {e}\n");
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    };

    let token = cfg.token.clone();

    // Shared loop handles, keyed by channel: the command handlers and each
    // loop's START watcher track their loop through it.
    let loop_child: handler::LoopChild =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let watched: Vec<config::LoopConfig> = cfg.loops.values().cloned().collect();

    for lc in &watched {
        eprintln!(
            "ralphd: loop `{}` → channel {} · {}",
            lc.name,
            lc.channel_id,
            lc.working_dir.display()
        );
        // Without a webhook the child runs with DISCORD_WEBHOOK cleared, so its
        // lifecycle posts go nowhere rather than into another loop's channel.
        if lc.webhook.is_none() {
            eprintln!(
                "ralphd: loop `{}` has no webhook — its lifecycle posts are off",
                lc.name
            );
        }
    }

    // Application-command interactions arrive without any privileged intents.
    let intents = GatewayIntents::empty();
    let mut client = match Client::builder(&token, intents)
        .event_handler(handler::Handler {
            cfg,
            loop_child: loop_child.clone(),
            autostarted: std::sync::atomic::AtomicBool::new(false),
        })
        .await
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ralphd: could not build client: {e}");
            std::process::exit(1);
        }
    };

    for lc in watched {
        // Watch for that loop's `ralph start` trigger file, using the client's
        // REST http so a local process can launch it without a Discord message.
        tokio::spawn(handler::watch_start(
            lc.clone(),
            loop_child.clone(),
            client.http.clone(),
        ));
        // Maintain its pinned live status card while it runs.
        tokio::spawn(card::watch_card(lc, loop_child.clone(), client.http.clone()));
    }

    eprintln!("ralphd: connecting…");
    if let Err(e) = client.start().await {
        eprintln!("ralphd: client error: {e}");
        std::process::exit(1);
    }
}
