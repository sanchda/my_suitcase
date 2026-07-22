//! The entire ralphd security model: a command is honored only from the one
//! configured channel AND the one configured user. Everything else is refused.

use crate::config::BotConfig;

/// True only when BOTH the channel and the user match the configured single tenant.
pub fn authorized(channel_id: u64, user_id: u64, cfg: &BotConfig) -> bool {
    channel_id == cfg.channel_id && user_id == cfg.user_id
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn cfg() -> BotConfig {
        BotConfig {
            token: "t".into(),
            guild_id: 1,
            channel_id: 100,
            user_id: 200,
            working_dir: PathBuf::from("."),
            state_dir: PathBuf::from(".ralph"),
            ralph_args: vec![],
        }
    }

    #[test]
    fn only_matching_channel_and_user_is_authorized() {
        let c = cfg();
        assert!(authorized(100, 200, &c));
        assert!(!authorized(999, 200, &c)); // wrong channel
        assert!(!authorized(100, 999, &c)); // wrong user
        assert!(!authorized(999, 999, &c));
    }
}
