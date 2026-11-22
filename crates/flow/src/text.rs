//! Text helpers used by blocks.

/// Splits text into chunks of about `size` characters, overlapping by
/// `overlap`, preferring to cut at a newline, period, comma or space within
/// `deviation` characters of the ideal cut. Works on characters, not bytes.
pub fn split(input: &str, size: usize, overlap: usize, deviation: usize) -> Vec<String> {
    let chars: Vec<char> = input.chars().collect();
    let size = size.max(1);
    let overlap = overlap.min(size.saturating_sub(1));
    let mut out = Vec::new();
    let mut start = 0;
    while chars.len() - start > size {
        let cut = find_break(&chars, start + size, deviation).clamp(start + 1, chars.len());
        out.push(chars[start..cut].iter().collect());
        start = if overlap == 0 {
            cut
        } else {
            let next = find_break(&chars, cut.saturating_sub(overlap), deviation);
            // Always make progress, and never skip text.
            if next > start && next <= cut {
                next
            } else {
                cut
            }
        };
    }
    out.push(chars[start..].iter().collect());
    out
}

fn find_break(chars: &[char], center: usize, radius: usize) -> usize {
    let center = center.min(chars.len());
    for b in ['\n', '.', ',', ' '] {
        for r in 0..=radius {
            if center + r < chars.len() && chars[center + r] == b {
                return center + r + 1;
            }
            if r <= center && center - r < chars.len() && chars[center - r] == b {
                return center - r + 1;
            }
        }
    }
    center
}

/// Removes `<think>…</think>` sections some models put in their answer.
pub fn strip_thinking(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<think>") {
        out.push_str(&rest[..i]);
        match rest[i..].find("</think>") {
            Some(j) => rest = &rest[i + j + "</think>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// The `<think>…</think>` sections of an answer, joined.
pub fn thinking(s: &str) -> String {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find("<think>") {
        let after = &rest[i + "<think>".len()..];
        match after.find("</think>") {
            Some(j) => {
                out.push(after[..j].trim().to_string());
                rest = &after[j + "</think>".len()..];
            }
            None => {
                out.push(after.trim().to_string());
                break;
            }
        }
    }
    out.retain(|t| !t.is_empty());
    out.join("\n\n")
}

/// Cuts text to `max` characters, saying so.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + &format!("\n[… cut at {max} characters]")
    }
}

/// Replaces `_INPUT_` in `prompt` with `input`, or appends the input if the
/// placeholder isn't there.
pub fn fill_prompt(prompt: &str, input: &str) -> String {
    if prompt.trim().is_empty() {
        input.to_string()
    } else if prompt.contains("_INPUT_") {
        prompt.replace("_INPUT_", input)
    } else if input.is_empty() {
        prompt.to_string()
    } else {
        format!("{prompt}\n\n{input}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_covers_everything_and_breaks_cleanly() {
        let text = "One sentence here. Another sentence there. And a third one.";
        let chunks = split(text, 20, 0, 10);
        assert!(chunks.len() >= 3);
        assert_eq!(chunks.concat(), text);
        assert_eq!(chunks[0], "One sentence here.");
        assert_eq!(split("short", 100, 0, 5), vec!["short"]);
        assert_eq!(split("", 10, 0, 5), vec![""]);
    }

    #[test]
    fn split_handles_overlap_and_unicode() {
        let text = "ééééé ééééé ééééé ééééé";
        let chunks = split(text, 8, 3, 2);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| !c.is_empty()));
    }

    #[test]
    fn strips_thinking() {
        assert_eq!(strip_thinking("<think>hmm</think>\nAnswer"), "Answer");
        assert_eq!(strip_thinking("A <think>x</think>B"), "A B");
        assert_eq!(strip_thinking("<think>never closed"), "");
        assert_eq!(thinking("<think> a </think>B<think>c</think>"), "a\n\nc");
        assert_eq!(truncate("abcdef", 3), "abc\n[… cut at 3 characters]");
    }

    #[test]
    fn fills_prompts() {
        assert_eq!(fill_prompt("Q: _INPUT_?", "x"), "Q: x?");
        assert_eq!(fill_prompt("Summarize.", "text"), "Summarize.\n\ntext");
        assert_eq!(fill_prompt("", "just this"), "just this");
    }
}
