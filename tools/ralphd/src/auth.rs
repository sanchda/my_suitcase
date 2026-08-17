//! The entire ralphd security model: a command is honored only from the one
//! configured user AND a channel that a configured loop claims. Everything else
//! is refused.

use crate::config::BotConfig;

pub fn authorized(channel_id: u64, user_id: u64, cfg: &BotConfig) -> bool {
    user_id == cfg.user_id && cfg.loops.contains_key(&channel_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LoopConfig;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn loop_at(channel_id: u64) -> LoopConfig {
        LoopConfig {
            name: "a".into(),
            channel_id,
            working_dir: PathBuf::from("."),
            state_dir: PathBuf::from(".ralph"),
            ralph_config: PathBuf::from(".ralph/ralph.toml"),
            ralph_args: vec![],
            webhook: None,
            autostart: false,
        }
    }

    fn cfg() -> BotConfig {
        BotConfig {
            token: "t".into(),
            guild_id: 1,
            user_id: 200,
            loops: HashMap::from([(100, loop_at(100)), (101, loop_at(101))]),
        }
    }

    #[test]
    fn only_a_known_loop_channel_and_the_one_user_is_authorized() {
        let c = cfg();
        assert!(authorized(100, 200, &c));
        assert!(authorized(101, 200, &c)); // a second loop's channel
        assert!(!authorized(999, 200, &c)); // channel with no loop
        assert!(!authorized(100, 999, &c)); // wrong user
        assert!(!authorized(999, 999, &c));
    }
}
