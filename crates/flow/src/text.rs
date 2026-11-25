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

/// Splits off a leading reasoning section that has a closing tag but no
/// opening one. Many reasoning models' chat templates open `<think>` in the
/// prompt, so the answer starts inside it: `reasoning…</think>answer`.
fn bare_close(s: &str) -> Option<(&str, &str)> {
    let close = s.find("</think>")?;
    match s.find("<think>") {
        Some(open) if open < close => None,
        _ => Some((&s[..close], &s[close + "</think>".len()..])),
    }
}

/// Removes `<think>…</think>` sections some models put in their answer
/// (including a leading one whose opening tag was in the prompt).
pub fn strip_thinking(s: &str) -> String {
    let s = bare_close(s).map_or(s, |(_, rest)| rest);
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
    if let Some((before, after)) = bare_close(s) {
        out.push(before.trim().to_string());
        rest = after;
    }
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
/// A rough token count for text: about four characters a token. Good enough
/// for context and usage figures; real tokenizers differ by model.
pub fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + &format!("\n[… cut at {max} characters]")
    }
}

/// Replaces `_INPUT_` in `prompt` with `input`, or appends the input if the
/// placeholder isn't there.
#[cfg(test)]
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

/// Memory saved during a run, read in templates as `{{name}}`.
pub type Memory = std::collections::HashMap<String, String>;

/// Whether `name` is a valid memory name: 1–40 letters, digits or `_`.
pub fn valid_memory_key(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Fills `{{name}}` from memory (empty if nothing is saved under it) and, if
/// `input` is given, `_INPUT_` with it, in one pass, so text that comes from
/// memory or the input is never itself searched for placeholders. Without
/// `_INPUT_` in the template, the input is added after it (as
/// [`fill_prompt`] does).
pub fn fill_template(template: &str, input: Option<&str>, memory: &Memory) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    let mut used_input = false;
    loop {
        let next_mem = rest.find("{{");
        let next_in = input.and(rest.find("_INPUT_"));
        let i = match (next_mem, next_in) {
            (None, None) => break,
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (Some(a), Some(b)) => a.min(b),
        };
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let (Some(inp), true) = (input, tail.starts_with("_INPUT_")) {
            out.push_str(inp);
            used_input = true;
            rest = &tail["_INPUT_".len()..];
            continue;
        }
        // `{{name}}`
        match tail[2..].find("}}") {
            Some(end) if valid_memory_key(tail[2..2 + end].trim()) => {
                let key = tail[2..2 + end].trim();
                out.push_str(memory.get(key).map(String::as_str).unwrap_or(""));
                rest = &tail[2 + end + 2..];
            }
            _ => {
                out.push_str("{{");
                rest = &tail[2..];
            }
        }
    }
    out.push_str(rest);
    match input {
        Some(inp) if !used_input => {
            if out.trim().is_empty() {
                inp.to_string()
            } else if inp.is_empty() {
                out
            } else {
                format!("{out}\n\n{inp}")
            }
        }
        _ => out,
    }
}

/// Replaces `_INPUT1_`, `_INPUT2_`, … with `values`, in one pass, so text
/// that arrives on one input is never rewritten by another.
#[cfg(test)]
pub fn fill_numbered(format: &str, values: &[String]) -> String {
    fill_numbered_with(format, values, &Memory::new())
}

/// [`fill_numbered`] that also fills `{{name}}` from memory, in the same pass.
pub fn fill_numbered_with(format: &str, values: &[String], memory: &Memory) -> String {
    let mut out = String::with_capacity(format.len());
    let mut rest = format;
    loop {
        let i = match (rest.find("_INPUT"), rest.find("{{")) {
            (None, None) => break,
            (Some(a), None) | (None, Some(a)) => a,
            (Some(a), Some(b)) => a.min(b),
        };
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(inner) = tail.strip_prefix("{{") {
            match inner.find("}}") {
                Some(end) if valid_memory_key(inner[..end].trim()) => {
                    out.push_str(
                        memory
                            .get(inner[..end].trim())
                            .map(String::as_str)
                            .unwrap_or(""),
                    );
                    rest = &inner[end + 2..];
                }
                _ => {
                    out.push_str("{{");
                    rest = inner;
                }
            }
            continue;
        }
        let after = &tail["_INPUT".len()..];
        let digits = after.chars().take_while(|c| c.is_ascii_digit()).count();
        let n: Option<usize> = after[..digits].parse().ok();
        match n {
            Some(n) if after[digits..].starts_with('_') && n >= 1 && n <= values.len() => {
                out.push_str(&values[n - 1]);
                rest = &after[digits + 1..];
            }
            _ => {
                out.push_str("_INPUT");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
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

        // (was split_handles_overlap_and_unicode)
        {
            let text = "ééééé ééééé ééééé ééééé";
            let chunks = split(text, 8, 3, 2);
            assert!(chunks.len() > 1);
            assert!(chunks.iter().all(|c| !c.is_empty()));
        }
    }

    #[test]
    fn strips_thinking() {
        assert_eq!(strip_thinking("<think>hmm</think>\nAnswer"), "Answer");
        assert_eq!(strip_thinking("A <think>x</think>B"), "A B");
        assert_eq!(strip_thinking("<think>never closed"), "");
        assert_eq!(thinking("<think> a </think>B<think>c</think>"), "a\n\nc");
        assert_eq!(truncate("abcdef", 3), "abc\n[… cut at 3 characters]");
        // The opening tag was in the prompt (Qwen3, DeepSeek-R1 templates).
        assert_eq!(
            strip_thinking("plan it first</think>\n\nThe answer"),
            "The answer"
        );
        assert_eq!(thinking("plan it first</think>The answer"), "plan it first");
        assert_eq!(
            strip_thinking("a <think>b</think> c </think> d"),
            "a  c </think> d"
        );
        assert_eq!(truncate("ab", 2), "ab");
    }

    #[test]
    fn fills_prompts() {
        assert_eq!(fill_prompt("Q: _INPUT_?", "x"), "Q: x?");
        assert_eq!(fill_prompt("Summarize.", "text"), "Summarize.\n\ntext");
        assert_eq!(fill_prompt("", "just this"), "just this");

        // (was templates_fill_memory_and_input_once)
        {
            let mut m = Memory::new();
            m.insert("notes".into(), "saw _INPUT_ and {{notes}}".into());
            assert_eq!(
                fill_template("Notes: {{notes}}\nTask: _INPUT_", Some("do {{notes}}"), &m),
                "Notes: saw _INPUT_ and {{notes}}\nTask: do {{notes}}"
            );
            // Unknown names are empty; things that aren't names are left alone.
            assert_eq!(
                fill_template("[{{missing}}] {{not a name}} {{", None, &m),
                "[] {{not a name}} {{"
            );
            // Without _INPUT_, the input goes after.
            assert_eq!(fill_template("Do this:", Some("x"), &m), "Do this:\n\nx");
            assert_eq!(fill_template("", Some("x"), &m), "x");
            assert_eq!(
                fill_template("{{ notes }}", None, &m),
                "saw _INPUT_ and {{notes}}"
            );
        }

        // (was fills_numbered_inputs_in_one_pass)
        {
            let v = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
            assert_eq!(
                fill_numbered("_INPUT1_ and _INPUT2_", &v(&["a", "b"])),
                "a and b"
            );
            // Text from input 2 that looks like a placeholder stays as it is.
            assert_eq!(
                fill_numbered("_INPUT1_|_INPUT2_", &v(&["x", "_INPUT1_"])),
                "x|_INPUT1_"
            );
            let ten: Vec<String> = (1..=10).map(|i| i.to_string()).collect();
            assert_eq!(fill_numbered("_INPUT10_-_INPUT1_", &ten), "10-1");
            assert_eq!(
                fill_numbered("_INPUT3_ _INPUT_ _INPUTx_ _INPUT", &v(&["a"])),
                "_INPUT3_ _INPUT_ _INPUTx_ _INPUT"
            );
        }
    }
}
