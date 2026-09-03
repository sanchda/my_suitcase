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
            crate::backlog::SCHEMA_MARKER,
            "- [ ] **12 —",
            "@opus — ",
            "Verify:",
            "free prose under a heading is not injected",
            "exactly two spaces",
            "ralph lint",
            "ralph brief",
        ] {
            assert!(REFERENCE.contains(required), "missing {required:?}");
        }
    }

    /// The bodies of every ```` ```markdown ```` block in the reference.
    fn markdown_examples(text: &str) -> Vec<String> {
        let mut examples = Vec::new();
        let mut current: Option<String> = None;
        for line in text.lines() {
            match current.as_mut() {
                Some(buffer) => {
                    if line.trim_start().starts_with("```") {
                        examples.push(std::mem::take(buffer));
                        current = None;
                    } else {
                        buffer.push_str(line);
                        buffer.push('\n');
                    }
                }
                None if line.trim_start() == "```markdown" => current = Some(String::new()),
                None => {}
            }
        }
        examples
    }

    /// The reference is printed verbatim by `ralph schema`, so an example that
    /// would not lint is advice the loop cannot follow.
    #[test]
    fn every_markdown_example_parses_without_errors() {
        let examples = markdown_examples(REFERENCE);
        assert!(examples.len() >= 2, "expected fenced examples to extract");
        for example in examples {
            let doc = crate::backlog::Document::parse(&example);
            assert!(!doc.has_errors(), "{example}\n{:?}", doc.issues);
        }
    }

    #[test]
    fn rejects_options_other_than_help() {
        assert!(run(&["--verbose".into()]).is_err());
    }
}
