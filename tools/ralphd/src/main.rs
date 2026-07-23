mod auth;
mod btw;
mod config;
mod format;
mod handler;
mod loop_pid;
mod model;
mod ralph;

use serenity::prelude::*;

const USAGE: &str = "\
ralphd — single-tenant Discord control bridge for the ralph loop

Usage:
  DISCORD_BOT_TOKEN=<token> ralphd [options] -- [ralph args...]

Options (each also settable via the environment):
  --guild <id>          Discord server (guild) id          [env RALPHD_GUILD_ID]
  --channel <id>        Channel commands are accepted in    [env RALPHD_CHANNEL_ID]
  --user <id>           The one authorized user id          [env RALPHD_USER_ID]
  --working-dir <path>  Repo the loop runs in (default: .)  [env RALPHD_WORKING_DIR]
  --autostart           Start the loop on connect            [env RALPHD_AUTOSTART]
  -h, --help            Show this help

Required environment:
  DISCORD_BOT_TOKEN     Bot token (env only, never a flag)

Everything after `--` is forwarded verbatim to `ralph` when you run /start.
Slash commands: /start /stop /model /status /next /backlog-add /backlog-edit /btw
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

    // Shared loop handle: the command handlers and the START watcher both track
    // the single loop through it.
    let loop_child: handler::LoopChild = std::sync::Arc::new(std::sync::Mutex::new(None));
    let watcher_cfg = cfg.clone();
    let watcher_loop_child = loop_child.clone();

    // Application-command interactions arrive without any privileged intents.
    let intents = GatewayIntents::empty();
    let mut client = match Client::builder(&token, intents)
        .event_handler(handler::Handler {
            cfg,
            loop_child,
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

    // Watch for the `ralph start` trigger file, using the client's REST http so a
    // local process can launch the tracked loop without a Discord message.
    tokio::spawn(handler::watch_start(
        watcher_cfg,
        watcher_loop_child,
        client.http.clone(),
    ));

    eprintln!("ralphd: connecting…");
    if let Err(e) = client.start().await {
        eprintln!("ralphd: client error: {e}");
        std::process::exit(1);
    }
}
