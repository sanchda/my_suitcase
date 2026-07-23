//! Built-in, hard-won lessons for authoring a per-project `PROMPT.md`.
//!
//! A curated companion to `ralph init`'s template: print it while crafting a
//! project's prompt so lessons learned across loops aren't rediscovered. Append
//! to `HINTS.md` as new lessons emerge.

use crate::R;

const HINTS: &str = include_str!("../HINTS.md");

pub fn run(args: &[String]) -> R<i32> {
    if args.is_empty() || matches!(args, [arg] if arg == "-h" || arg == "--help") {
        print!("{HINTS}");
        return Ok(0);
    }
    Err("`ralph hints` takes no options".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_cover_the_core_lessons() {
        for required in [
            "Verification is the whole game",
            "Any process the agent starts is the agent's to stop",
            "The end-of-turn summary is the only handoff",
            "git add -A",
            "iteration_timeout",
            "ralph schema",
        ] {
            assert!(HINTS.contains(required), "missing {required:?}");
        }
    }

    #[test]
    fn rejects_options_other_than_help() {
        assert!(run(&["--verbose".into()]).is_err());
    }
}
