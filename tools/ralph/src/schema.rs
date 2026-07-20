//! Built-in reference for the backlog format.

use crate::R;

const REFERENCE: &str = include_str!("../BACKLOG.schema.md");

pub fn run(args: &[String]) -> R<i32> {
    if args.is_empty() || matches!(args, [arg] if arg == "-h" || arg == "--help") {
        print!("{REFERENCE}");
        return Ok(0);
    }
    Err("`ralph schema` takes no options".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_covers_authoring_routing_and_inspection() {
        for required in [
            "<!-- ralph-backlog: v1 -->",
            "- [ ] **12 —",
            "Verify:",
            "free prose under a heading is not injected",
            "exactly two spaces",
            "ralph lint",
            "ralph brief",
        ] {
            assert!(REFERENCE.contains(required), "missing {required:?}");
        }
    }

    #[test]
    fn rejects_options_other_than_help() {
        assert!(run(&["--verbose".into()]).is_err());
    }
}
