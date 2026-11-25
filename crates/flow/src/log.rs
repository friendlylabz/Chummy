//! What a run records about each step, for run logs and the chat's step
//! view. Values are kept as readable text, cut to a size that keeps logs of
//! long runs manageable (the head and the end are kept, since the end of a
//! growing exchange is usually what changed).

use serde::{Deserialize, Serialize};

use crate::engine::Value;
use crate::exchange::Exchange;

/// Most characters of one value kept in a log entry.
pub const LOG_VALUE_CHARS: usize = 6000;

/// A value as a log shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedValue {
    /// "text", "list" or "exchange".
    pub kind: String,
    /// Readable text. Lists are numbered; exchanges are a transcript with
    /// roles and tool calls.
    pub text: String,
    /// Characters in the whole value, before cutting.
    pub chars: usize,
    /// Characters left out of `text`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cut: usize,
    /// Items in a list, or messages in an exchange.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<usize>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// One port's value in a step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortLog {
    pub port: String,
    pub value: LoggedValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Done,
    /// It failed, and its Failed output carried the error on.
    Caught,
    /// It failed and stopped the run.
    Failed,
    /// The run was stopped (by the user or a limit) while it ran.
    Stopped,
}

/// One block run: what went in and what came out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepLog {
    /// The run's step number when the block started (1, 2, …). Steps inside
    /// a function come between the function's own start and its end.
    pub n: u32,
    /// Path of the block: `<function block>/<block>` inside functions.
    pub block: String,
    pub label: String,
    /// The block type, e.g. "llm".
    pub kind: String,
    /// For a function block: the function's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    pub inputs: Vec<PortLog>,
    pub outputs: Vec<PortLog>,
    pub status: StepStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Unix time in milliseconds when it started.
    pub started_at: u64,
    /// How long it took, in milliseconds.
    pub ms: u64,
}

/// One call to a model, exactly as sent and answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelLog {
    pub block: String,
    /// 1 for the first call; more when the block retried after a bad call.
    pub attempt: u32,
    /// The model connection the block chose; `None` means the agent's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// "native", "prompted" or "off".
    pub tool_mode: String,
    /// Tools offered to the model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// The conversation sent.
    pub sent: LoggedValue,
    pub answer: LoggedValue,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reasoning: String,
    /// Tool calls in the answer, described.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<String>,
    pub started_at: u64,
    pub ms: u64,
}

/// Cuts `s` to at most `max` characters, keeping a third from the start and
/// the rest from the end. Returns the text and how many characters went.
pub fn cut(s: &str, max: usize) -> (String, usize) {
    let n = s.chars().count();
    if n <= max {
        return (s.to_string(), 0);
    }
    let head = max / 3;
    let tail = max - head;
    let left = n - head - tail;
    let start: String = s.chars().take(head).collect();
    let end: String = s.chars().skip(n - tail).collect();
    (
        format!("{start}\n… ({left} characters left out) …\n{end}"),
        left,
    )
}

pub fn exchange_text(e: &Exchange) -> String {
    e.messages
        .iter()
        .map(|m| {
            let mut s = format!("[{}]\n{}", m.role.label(), m.text.trim());
            for c in &m.calls {
                s.push_str(&format!("\n→ calls {}({})", c.name, c.arguments));
            }
            s
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A value for a log, cut to `max` characters.
pub fn logged(v: &Value, max: usize) -> LoggedValue {
    let (kind, full, items) = match v {
        Value::Text(t) => ("text", t.clone(), None),
        Value::List(l) => (
            "list",
            l.iter()
                .enumerate()
                .map(|(i, x)| format!("{}. {}", i + 1, x))
                .collect::<Vec<_>>()
                .join("\n"),
            Some(l.len()),
        ),
        Value::Exchange(e) => ("exchange", exchange_text(e), Some(e.messages.len())),
    };
    let chars = full.chars().count();
    let (text, cut) = cut(&full, max);
    LoggedValue {
        kind: kind.into(),
        text,
        chars,
        cut,
        items,
    }
}

/// Unix time now, in milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exchange::{ExMessage, Role};

    #[test]
    fn values_read_well() {
        let l = logged(&Value::List(vec!["x".into(), "y".into()]), 100);
        assert_eq!(l.text, "1. x\n2. y");
        assert_eq!(l.items, Some(2));
        let mut e = Exchange::default();
        e.messages.push(ExMessage::new(Role::User, "Hi"));
        let l = logged(&Value::Exchange(e), 100);
        assert_eq!(l.kind, "exchange");
        assert!(l.text.contains("Hi"), "{}", l.text);

        // (was cuts_keep_both_ends)
        {
            let s = "a".repeat(100) + &"b".repeat(100);
            let (t, left) = cut(&s, 30);
            assert_eq!(left, 170);
            assert!(t.starts_with(&"a".repeat(10)));
            assert!(t.ends_with(&"b".repeat(20)));
            assert_eq!(cut("short", 30), ("short".to_string(), 0));
        }
    }
}
