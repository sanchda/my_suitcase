//! Discord message hygiene for model-generated text: split long output into
//! multiple messages at line boundaries, never inside a fenced code block, and
//! cap the number of messages. A fence that itself exceeds one message is
//! closed at the split and reopened (with its language tag) in the next chunk.

/// Discord's message-content cap.
pub const DISCORD_LIMIT: usize = 2000;
/// Most messages one `/btw` result may produce; the rest is dropped with a note.
pub const MAX_CHUNKS: usize = 4;

/// Split `text` into chunks of at most `limit` chars, breaking only at line
/// boundaries (a single over-long line is hard-split), keeping code fences
/// well-formed in every chunk.
pub fn chunk_message(text: &str, limit: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    // The opening fence line (e.g. "```rust") when we're inside a fence.
    let mut fence: Option<String> = None;

    // Close-and-flush `current`, reopening the fence in the next chunk.
    let flush = |current: &mut String, chunks: &mut Vec<String>, fence: &Option<String>| {
        if let Some(f) = fence {
            current.push_str("\n```");
            chunks.push(std::mem::take(current));
            current.push_str(f);
            current.push('\n');
        } else {
            chunks.push(std::mem::take(current));
        }
    };

    for line in text.lines() {
        let trimmed = line.trim_start();
        // Reserve room to close a fence ("\n```") if we're inside one.
        let reserve = if fence.is_some() { 4 } else { 0 };
        let needed = line.chars().count() + usize::from(!current.is_empty());
        if !current.is_empty() && current.chars().count() + needed + reserve > limit {
            flush(&mut current, &mut chunks, &fence);
        }
        // A single line longer than the limit: hard-split by chars.
        if line.chars().count() + reserve > limit {
            let mut rest: Vec<char> = line.chars().collect();
            while rest.len() + reserve > limit {
                let head: String = rest.drain(..limit - reserve - 1).collect();
                if !current.is_empty() {
                    current.push('\n');
                }
                current.push_str(&head);
                flush(&mut current, &mut chunks, &fence);
            }
            let tail: String = rest.into_iter().collect();
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(&tail);
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
        if trimmed.starts_with("```") {
            fence = match fence {
                Some(_) => None,
                None => Some(trimmed.to_string()),
            };
        }
    }
    if !current.trim().is_empty() || chunks.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Keep at most `max` chunks, replacing the overflow with an explicit note.
pub fn cap_chunks(mut chunks: Vec<String>, max: usize) -> Vec<String> {
    if chunks.len() > max {
        let dropped = chunks.len() - max;
        chunks.truncate(max);
        if let Some(last) = chunks.last_mut() {
            last.push_str(&format!("\n\n… ({dropped} more message(s) omitted)"));
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every chunk must be within the limit and contain balanced fences.
    fn assert_well_formed(chunks: &[String], limit: usize) {
        for c in chunks {
            assert!(
                c.chars().count() <= limit,
                "chunk over limit ({}): {c:?}",
                c.chars().count()
            );
            let fences = c
                .lines()
                .filter(|l| l.trim_start().starts_with("```"))
                .count();
            assert_eq!(fences % 2, 0, "unbalanced fence in chunk: {c:?}");
        }
    }

    #[test]
    fn short_text_is_one_chunk() {
        let chunks = chunk_message("hello\nworld", 2000);
        assert_eq!(chunks, vec!["hello\nworld"]);
    }

    #[test]
    fn splits_at_line_boundaries() {
        let text = (0..100).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let chunks = chunk_message(&text, 120);
        assert!(chunks.len() > 1);
        assert_well_formed(&chunks, 120);
        // No line is torn in half.
        for c in &chunks {
            for l in c.lines() {
                assert!(l.starts_with("line "), "torn line: {l:?}");
            }
        }
        // Nothing lost.
        assert_eq!(chunks.join("\n"), text);
    }

    #[test]
    fn fence_is_closed_and_reopened_across_chunks() {
        let mut text = String::from("intro\n```rust\n");
        for i in 0..60 {
            text.push_str(&format!("let x{i} = {i};\n"));
        }
        text.push_str("```\ndone");
        let chunks = chunk_message(&text, 200);
        assert!(chunks.len() > 1);
        assert_well_formed(&chunks, 200);
        // Continuation chunks reopen with the language tag.
        assert!(chunks[1].starts_with("```rust\n"), "{:?}", &chunks[1][..20]);
        // The code lines all survive.
        let rejoined = chunks.join("\n");
        assert!(rejoined.contains("let x59 = 59;"));
        assert!(rejoined.contains("done"));
    }

    #[test]
    fn over_long_single_line_is_hard_split() {
        let text = "x".repeat(5000);
        let chunks = chunk_message(&text, 2000);
        assert!(chunks.len() >= 3);
        assert_well_formed(&chunks, 2000);
        let total: usize = chunks.iter().map(|c| c.chars().count()).sum();
        assert_eq!(total, 5000);
    }

    #[test]
    fn cap_names_what_it_drops() {
        let chunks: Vec<String> = (0..7).map(|i| format!("c{i}")).collect();
        let capped = cap_chunks(chunks, 4);
        assert_eq!(capped.len(), 4);
        assert!(capped[3].contains("3 more message(s) omitted"));
        // Under the cap: untouched.
        let ok = cap_chunks(vec!["a".into()], 4);
        assert_eq!(ok, vec!["a"]);
    }
}
