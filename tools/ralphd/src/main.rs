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

const USAGE: &str = r##"ralphd — Discord control bridge for ralph loops, one channel per loop

One always-on process watching one Discord guild. Each configured channel drives
one `ralph` loop in one repo, and the channel a command arrives in is what selects
the loop — there is no --repo argument. Every command shells out to the `ralph`
CLI and ralphd only *reads* `.ralph/`, so it is never load-bearing: anything you
can do from Discord you can do from a terminal, and killing ralphd loses nothing.

USAGE
  DISCORD_BOT_TOKEN=<token> ralphd [--config <path>]
  DISCORD_BOT_TOKEN=<token> ralphd --guild <id> --channel <id> --user <id> \
                                   [--working-dir <path>] [--autostart] \
                                   [-- <ralph args…>]

  The first form drives many loops from a TOML file; the second is the
  single-loop shorthand, kept working unchanged.

ENVIRONMENT
  DISCORD_BOT_TOKEN   Required. Bot token — env only, never a flag.
  DISCORD_WEBHOOK     Optional. Inherited by a lone loop that declares no
                      `webhook` of its own; with two or more loops it is never
                      inherited, since one ambient webhook would funnel every
                      loop's lifecycle posts into a single channel.

CONFIG FILE
  Precedence, first match wins:
    1. --config <path>, or RALPHD_CONFIG
    2. the single-loop flag form, when --channel / RALPHD_CHANNEL_ID is set
    3. ~/.config/ralphd.toml, if it exists
  The flag form beating the default path is deliberate — a stale config file
  cannot hijack a launch that works today.

    guild = 123        # the one guild
    user  = 456        # the one authorized user

    [[loop]]
    name      = "number-grove"
    channel   = 111
    dir       = "/home/me/dev/number_grove"
    args      = ["--model", "sonnet"]                  # optional
    webhook   = "https://discord.com/api/webhooks/…"   # optional, per loop
    autostart = false                                  # optional

  ralphd.toml.example is a commented copy of every accepted key. Unknown keys
  are a hard parse error, and two loops may not claim the same channel.

OPTIONS (each also settable via the environment; the flag wins)
  --config <path>       Multi-loop TOML config             [RALPHD_CONFIG]
  --guild <id>          Discord server (guild) id          [RALPHD_GUILD_ID]
  --channel <id>        Channel commands are accepted in   [RALPHD_CHANNEL_ID]
  --user <id>           The one authorized user id         [RALPHD_USER_ID]
  --working-dir <path>  Repo the loop runs in (default .)  [RALPHD_WORKING_DIR]
  --autostart           Start the loop on connect          [RALPHD_AUTOSTART]
  -h, --help            Show this help

  The last four describe one loop; use the config file for more than one.
  Everything after a bare `--` is forwarded verbatim to `ralph` on /start.

COMMANDS (each acts on the loop that owns the channel you type it in)
  /start [model]                        ralph <profile args> [--model …]
  /stop [now]                           ralph stop [--now]
  /model <tier>                         ralph model <tier>
  /status, /next                        ralph status --json
  /add <title> [verify] [id] [under]    ralph add [--under P] [id] <title> …
  /drop <id> [recursive]                ralph drop <id> [--recursive]
  /done <id>                            ralph done <id>
  /uncheck <id>                         ralph uncheck <id>
  /backlog-edit <id> <title> <verify>   ralph backlog edit …
  /msg <message> [model] [new]          ralph msg [--new] [--model …] <text>

IN THE CHANNEL
  One pinned status card per loop, edited in place every 30s while it runs and
  closed with a final past-tense edit when it ends. A loop ralphd spawned that
  exits abnormally gets a post with the abort reason and Start again / Start on
  opus buttons. Invite the bot with the `bot` and `applications.commands`
  scopes; pinning the card also needs the Manage Messages permission.

TRAPS
  Run exactly ONE ralphd per guild. Registering slash commands replaces the
  guild's entire command set, so two instances silently unregister each other.

See tools/ralphd/README.md for the rest.
"##;

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
        tokio::spawn(card::watch_card(lc, loop_child.clone(), client.http.clone()));
    }

    eprintln!("ralphd: connecting…");
    if let Err(e) = client.start().await {
        eprintln!("ralphd: client error: {e}");
        std::process::exit(1);
    }
}
