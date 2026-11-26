//! A run as people see it in a channel: what it says along the way, the
//! questions and answers, how it ended and its figures, the same as the
//! app's chat shows (no logs, no pane output).

use chummy_flow::Event;
use chummy_session::{RunSummary, Said, Update};

/// One run's response, built from the session's updates.
#[derive(Debug, Clone)]
pub struct RunView {
    pub run_id: String,
    parts: Vec<Said>,
    /// Replies came as they happened (otherwise they come with the saved
    /// response).
    live: bool,
    summary: Option<RunSummary>,
    done: bool,
}

/// Something new to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// What the agent said.
    Reply(String),
    Question(String),
    Answer(String),
    /// How the run ended, when that needs saying ("Stopped.", "Paused…").
    Notice(String),
    /// Its figures, once it's done (time, model calls, steps…).
    Figures(String),
}

impl RunView {
    pub fn new(run_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            parts: vec![],
            live: false,
            summary: None,
            done: false,
        }
    }

    pub fn done(&self) -> bool {
        self.done
    }

    pub fn summary(&self) -> Option<&RunSummary> {
        self.summary.as_ref()
    }

    /// Takes in an update (others' are ignored) and returns what it adds.
    pub fn update(&mut self, u: &Update) -> Vec<Line> {
        let mut out = vec![];
        match u {
            Update::RunEvent(e) if e.run_id == self.run_id => {
                if let Event::Reply { text, .. } = &e.event {
                    self.live = true;
                    Said::push(&mut self.parts, "assistant", text);
                    out.push(Line::Reply(text.clone()));
                }
            }
            Update::RunSaid(s) if s.run_id == self.run_id => {
                Said::push(&mut self.parts, &s.role, &s.text);
                out.push(match s.role.as_str() {
                    "question" => Line::Question(s.text.clone()),
                    _ => Line::Answer(s.text.clone()),
                });
            }
            Update::ChatMessage(m)
                if m.message
                    .run
                    .as_ref()
                    .is_some_and(|r| r.run_id == self.run_id) =>
            {
                self.summary = m.message.run.clone();
                match m.message.role.as_str() {
                    "notice" => {
                        self.parts.push(Said {
                            role: "notice".into(),
                            text: m.message.text.clone(),
                        });
                        out.push(Line::Notice(m.message.text.clone()));
                    }
                    // Replies the run didn't send along the way.
                    "assistant" if !self.live => {
                        Said::push(&mut self.parts, "assistant", &m.message.text);
                        out.push(Line::Reply(m.message.text.clone()));
                    }
                    _ => {}
                }
            }
            Update::ChatRunUpdated(r) if r.summary.run_id == self.run_id => {
                self.summary = Some(r.summary.clone());
            }
            Update::RunFinished(f) if f.run_id == self.run_id => {
                self.done = true;
                if let Some(s) = &self.summary {
                    out.push(Line::Figures(figures(s)));
                }
            }
            _ => {}
        }
        out
    }

    /// The whole response so far, as one message (for channels that edit
    /// a message as the run goes).
    pub fn text(&self) -> String {
        let mut lines: Vec<String> = self
            .parts
            .iter()
            .map(|p| match p.role.as_str() {
                "question" => format!("Question: {}", p.text),
                "answer" => format!("Answer: {}", p.text),
                "notice" => format!("({})", p.text.trim()),
                _ => p.text.clone(),
            })
            .collect();
        if self.done
            && let Some(s) = &self.summary
        {
            lines.push(figures(s));
        }
        lines.join("\n\n")
    }
}

/// Splits a message into parts of at most `max` bytes, at paragraph breaks
/// where it can, else at line breaks or spaces, else anywhere (on a
/// character boundary). For channels whose messages have a size limit.
pub fn split(text: &str, max: usize) -> Vec<String> {
    let max = max.max(16);
    let mut out = vec![];
    let mut rest = text;
    while rest.len() > max {
        let mut cut = max;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let head = &rest[..cut];
        let at = ["\n\n", "\n", " "]
            .iter()
            .find_map(|sep| {
                head.rfind(sep)
                    .filter(|i| *i > max / 2)
                    .map(|i| i + sep.len())
            })
            .unwrap_or(cut);
        out.push(rest[..at].trim_end().to_string());
        rest = rest[at..].trim_start_matches('\n');
    }
    if !rest.is_empty() || out.is_empty() {
        out.push(rest.to_string());
    }
    out
}

/// A run's figures, worded as the app's chat words them.
pub fn figures(r: &RunSummary) -> String {
    let n = |x: u64, one: &str| format!("{} {one}{}", group(x), if x == 1 { "" } else { "s" });
    let pct = |a: u64, b: u64| {
        let v = a as f64 / b as f64 * 100.0;
        if v > 0.0 && v < 1.0 {
            "<1%".to_string()
        } else {
            format!("{}%", v.round())
        }
    };
    let mut parts = vec![];
    if r.ms > 0 {
        parts.push(stopwatch(r.ms));
    }
    parts.push(n(r.llm_calls.into(), "model call"));
    if r.tool_calls > 0 {
        parts.push(n(r.tool_calls.into(), "tool call"));
    }
    parts.push(n(r.steps.into(), "step"));
    if r.tokens > 0 {
        parts.push(format!("{} used", n(r.tokens, "token")));
    }
    if r.context_window > 0 {
        parts.push(format!(
            "context {}",
            pct(r.context_tokens, r.context_window.into())
        ));
    }
    if let Some(allowed) = r.daily_allowed.filter(|a| *a > 0) {
        parts.push(format!("daily usage {}", pct(r.daily_used, allowed)));
    }
    parts.join(" · ")
}

/// A duration as minutes, seconds and tenths (`7.1s`, `1:05.3`).
pub fn stopwatch(ms: u64) -> String {
    let tenths = ms / 100;
    let t = tenths % 10;
    let total = tenths / 10;
    let (s, m, h) = (total % 60, (total / 60) % 60, total / 3600);
    match (h, m) {
        (0, 0) => format!("{s}.{t}s"),
        (0, _) => format!("{m}:{s:02}.{t}"),
        _ => format!("{h}:{m:02}:{s:02}.{t}"),
    }
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chummy_session::{ChatMessage, ChatMessageEvent, RunEvent, RunFinished, RunSaid};

    fn summary(run: &str) -> RunSummary {
        RunSummary {
            run_id: run.into(),
            status: "completed".into(),
            steps: 55,
            llm_calls: 2,
            tool_calls: 0,
            ms: 7_100,
            tokens: 1_234,
            context_tokens: 30,
            context_window: 8192,
            daily_used: 0,
            daily_allowed: None,
        }
    }

    fn message(run: &str, role: &str, text: &str) -> Update {
        Update::ChatMessage(ChatMessageEvent {
            agent_id: "a".into(),
            chat_id: "c".into(),
            message: ChatMessage {
                id: "m".into(),
                role: role.into(),
                text: text.into(),
                at: 0,
                from: None,
                run: Some(summary(run)),
            },
        })
    }

    fn finished(run: &str) -> Update {
        Update::RunFinished(RunFinished {
            run_id: run.into(),
            agent_id: None,
            chat_id: None,
            outcome: chummy_flow::RunOutcome {
                status: chummy_flow::RunStatus::Completed,
                replies: vec![],
                steps: 0,
                llm_calls: 0,
                tool_calls: 0,
                waiting: vec![],
                usage: Default::default(),
                paused: None,
            },
        })
    }

    #[test]
    fn a_run_reads_as_one_message() {
        let mut v = RunView::new("r1");
        let reply = |text: &str| {
            Update::RunEvent(RunEvent {
                run_id: "r1".into(),
                event: Event::Reply {
                    block: "out".into(),
                    text: text.into(),
                },
            })
        };
        assert_eq!(
            v.update(&reply("Looking.")),
            [Line::Reply("Looking.".into())]
        );
        // Another run's updates are someone else's.
        assert!(v.update(&message("r2", "notice", "Stopped.")).is_empty());
        let said = Update::RunSaid(RunSaid {
            run_id: "r1".into(),
            role: "question".into(),
            text: "Which one?".into(),
        });
        assert_eq!(v.update(&said), [Line::Question("Which one?".into())]);
        assert!(v.update(&reply("Done.")).len() == 1);
        // The saved response repeats replies already shown: not again.
        assert!(
            v.update(&message("r1", "assistant", "Looking.\n\nDone."))
                .is_empty()
        );
        assert_eq!(
            v.update(&finished("r1")),
            [Line::Figures(
                "7.1s · 2 model calls · 55 steps · 1,234 tokens used · context <1%".into()
            )]
        );
        assert!(v.done());
        assert_eq!(
            v.text(),
            "Looking.\n\nQuestion: Which one?\n\nDone.\n\n7.1s · 2 model calls · 55 steps · 1,234 tokens used · context <1%"
        );
    }

    #[test]
    fn replies_not_seen_along_the_way_come_with_the_response() {
        let mut v = RunView::new("r1");
        assert_eq!(
            v.update(&message("r1", "assistant", "All done.")),
            [Line::Reply("All done.".into())]
        );
        assert_eq!(
            v.update(&message("r1", "notice", "Stopped.")),
            [Line::Notice("Stopped.".into())]
        );
        assert_eq!(stopwatch(65_300), "1:05.3");
    }

    #[test]
    fn long_messages_split_at_paragraphs() {
        let text = format!(
            "{}\n\n{}\n{}",
            "a".repeat(30),
            "b".repeat(30),
            "c".repeat(30)
        );
        let parts = split(&text, 70);
        assert_eq!(
            parts,
            [
                format!("{}\n\n{}", "a".repeat(30), "b".repeat(30)),
                "c".repeat(30)
            ]
        );
        assert!(parts.iter().all(|p| p.len() <= 70));
        // No break to use: cut on a character boundary.
        let long = "é".repeat(50);
        let parts = split(&long, 33);
        assert!(parts.iter().all(|p| p.len() <= 33) && parts.concat() == long);
        assert_eq!(split("short", 100), ["short"]);
        assert_eq!(split("", 100), [""]);
        assert_eq!(stopwatch(3_725_000), "1:02:05.0");
    }
}
