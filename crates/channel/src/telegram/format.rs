//! Agents write Markdown; Telegram shows a small set of HTML tags. This
//! turns the common parts of Markdown (bold, italics, strikethrough, code,
//! code blocks, links, headings) into those tags and escapes everything
//! else, so any text gives HTML Telegram accepts: what isn't understood
//! stays as it was written. (Telegram's own "MarkdownV2" would need most
//! punctuation escaped, and refuses the message when one isn't.)

/// `md` as Telegram HTML.
pub fn html(md: &str) -> String {
    let mut out = String::with_capacity(md.len() + 16);
    let mut code: Option<String> = None;
    let mut lines = md.split('\n').peekable();
    while let Some(line) = lines.next() {
        let last = lines.peek().is_none();
        let fence = line.trim_start().strip_prefix("```");
        match (&mut code, fence) {
            (None, Some(lang)) => {
                let lang: String = lang
                    .trim()
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '+' | '_'))
                    .collect();
                if lang.is_empty() {
                    out.push_str("<pre><code>");
                } else {
                    out.push_str(&format!("<pre><code class=\"language-{lang}\">"));
                }
                code = Some(String::new());
                continue;
            }
            (Some(body), Some(_)) => {
                out.push_str(&escape(body.trim_end_matches('\n')));
                out.push_str("</code></pre>");
                code = None;
            }
            (Some(body), None) => {
                body.push_str(line);
                body.push('\n');
                continue;
            }
            (None, None) => out.push_str(&block_line(line)),
        }
        if !last {
            out.push('\n');
        }
    }
    // A block that wasn't closed (a long message split in the middle).
    if let Some(body) = code {
        out.push_str(&escape(body.trim_end_matches('\n')));
        out.push_str("</code></pre>");
    }
    out
}

/// One line outside code blocks: headings are bold, the rest inline.
fn block_line(line: &str) -> String {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
        return format!("<b>{}</b>", inline(t[hashes..].trim()));
    }
    inline(line)
}

/// Escapes text for Telegram's HTML.
pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            c => o.push(c),
        }
    }
    o
}

const MARKS: [(&str, &str); 5] = [
    ("**", "b"),
    ("__", "b"),
    ("~~", "s"),
    ("*", "i"),
    ("_", "i"),
];

fn inline(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Code: as it is, up to the next backtick.
        if c == '`'
            && let Some(end) = (i + 1..chars.len()).find(|&j| chars[j] == '`')
            && end > i + 1
        {
            let body: String = chars[i + 1..end].iter().collect();
            out.push_str(&format!("<code>{}</code>", escape(&body)));
            i = end + 1;
            continue;
        }
        // Links: [text](https://…).
        if c == '['
            && let Some((text, url, end)) = link(&chars, i)
        {
            out.push_str(&format!(
                "<a href=\"{}\">{}</a>",
                escape(&url),
                inline(&text)
            ));
            i = end;
            continue;
        }
        if let Some((tag, body, end)) = emphasis(&chars, i) {
            out.push_str(&format!("<{tag}>{}</{tag}>", inline(&body)));
            i = end;
            continue;
        }
        out.push_str(&escape(&c.to_string()));
        i += 1;
    }
    out
}

fn starts_with(chars: &[char], at: usize, s: &str) -> bool {
    (at..).zip(s.chars()).all(|(k, c)| chars.get(k) == Some(&c))
}

/// `**bold**`, `_italic_`… at `i`: the tag, what's inside, and where it
/// ends. The text inside doesn't start or end with a space; `_` marks only
/// count at the edges of words (so snake_case stays as it is).
fn emphasis(chars: &[char], i: usize) -> Option<(&'static str, String, usize)> {
    for (mark, tag) in MARKS {
        if !starts_with(chars, i, mark) {
            continue;
        }
        let n = mark.chars().count();
        let word = |k: usize| chars.get(k).is_some_and(|c| c.is_alphanumeric());
        if mark.starts_with('_') && i > 0 && word(i - 1) {
            return None;
        }
        let first = chars.get(i + n)?;
        if first.is_whitespace() {
            return None;
        }
        let mut j = i + n + 1;
        while j + n <= chars.len() {
            if starts_with(chars, j, mark)
                && !chars[j - 1].is_whitespace()
                && !(mark.starts_with('_') && word(j + n))
                // `**` isn't the end of a `*`.
                && !(n == 1 && chars.get(j + 1) == Some(&chars[j]))
            {
                let body: String = chars[i + n..j].iter().collect();
                return Some((tag, body, j + n));
            }
            j += 1;
        }
        return None;
    }
    None
}

/// `[text](url)` at `i`, with a web address: its text, address and end.
fn link(chars: &[char], i: usize) -> Option<(String, String, usize)> {
    let close = (i + 1..chars.len()).find(|&j| chars[j] == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = (close + 2..chars.len()).find(|&j| chars[j] == ')')?;
    let text: String = chars[i + 1..close].iter().collect();
    let url: String = chars[close + 2..end].iter().collect();
    let ok = (url.starts_with("https://") || url.starts_with("http://"))
        && !url.chars().any(char::is_whitespace);
    (ok && !text.is_empty()).then_some((text, url, end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_reads_as_telegram_html() {
        assert_eq!(html("plain & <simple>"), "plain &amp; &lt;simple&gt;");
        assert_eq!(
            html("**bold** and *it* and ~~gone~~"),
            "<b>bold</b> and <i>it</i> and <s>gone</s>"
        );
        assert_eq!(html("use `a<b>` here"), "use <code>a&lt;b&gt;</code> here");
        assert_eq!(html("snake_case_name stays"), "snake_case_name stays");
        assert_eq!(html("_whole_ word"), "<i>whole</i> word");
        assert_eq!(html("* a list item"), "* a list item");
        assert_eq!(html("2 * 3 * 4"), "2 * 3 * 4");
        assert_eq!(html("**unclosed"), "**unclosed");
        assert_eq!(html("# Title\nbody"), "<b>Title</b>\nbody");
        assert_eq!(
            html("see [the docs](https://example.org/a?b=1&c=2)"),
            "see <a href=\"https://example.org/a?b=1&amp;c=2\">the docs</a>"
        );
        assert_eq!(html("[x](javascript:alert(1))"), "[x](javascript:alert(1))");
        assert_eq!(
            html("```rust\nfn main() { a < b }\n```\nafter"),
            "<pre><code class=\"language-rust\">fn main() { a &lt; b }</code></pre>\nafter"
        );
        assert_eq!(
            html("```\nnever closed"),
            "<pre><code>never closed</code></pre>"
        );
        assert_eq!(html("**bold _and it_**"), "<b>bold <i>and it</i></b>");
    }
}
