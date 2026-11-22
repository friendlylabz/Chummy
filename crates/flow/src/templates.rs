//! Starting points offered when creating a loop. Nothing is installed until
//! the user picks one.

use serde::Serialize;
use serde_json::json;

use crate::format::{Block, Link, LoopFile, PortRef, Position};

#[derive(Debug, Clone, Serialize)]
pub struct Template {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub file: LoopFile,
}

fn b(id: &str, kind: &str, label: &str, x: f64, y: f64, params: serde_json::Value) -> Block {
    Block {
        id: id.into(),
        kind: kind.into(),
        label: label.into(),
        position: Position { x, y },
        params,
    }
}

fn l(a: &str, ap: &str, c: &str, cp: &str) -> Link {
    Link {
        from: PortRef {
            block: a.into(),
            port: ap.into(),
        },
        to: PortRef {
            block: c.into(),
            port: cp.into(),
        },
    }
}

const CONTEXT_FORMAT: &str = "Conversation so far:\n_INPUT1_\n\nThe user now says:\n_INPUT2_";

pub fn templates() -> Vec<Template> {
    let mut blank = LoopFile::new("New loop");
    blank.blocks = vec![
        b("message", "input", "Message", 0.0, 0.0, json!({})),
        b("reply", "output", "Reply", 420.0, 0.0, json!({})),
    ];

    let mut chat = LoopFile::new("Simple chat");
    chat.description = "Answers each message with the recent conversation as context.".into();
    chat.blocks = vec![
        b(
            "history",
            "history",
            "Conversation",
            0.0,
            0.0,
            json!({ "max_messages": 10 }),
        ),
        b("message", "input", "Message", 0.0, 180.0, json!({})),
        b(
            "context",
            "combine",
            "Context",
            300.0,
            60.0,
            json!({ "inputs": 2, "format": CONTEXT_FORMAT }),
        ),
        b(
            "answer",
            "llm",
            "Answer",
            600.0,
            60.0,
            json!({
                "prompt": "_INPUT_\n\nWrite the assistant's reply.",
                "system": "You are a helpful assistant. Answer clearly and briefly.",
                "max_tokens": 1024
            }),
        ),
        b("reply", "output", "Reply", 900.0, 60.0, json!({})),
    ];
    chat.links = vec![
        l("history", "out", "context", "in1"),
        l("message", "out", "context", "in2"),
        l("context", "out", "answer", "in"),
        l("answer", "out", "reply", "in"),
    ];

    let mut check = LoopFile::new("Draft and self-check");
    check.description =
        "Drafts a reply, asks the model to check it, and improves it until the check says YES."
            .into();
    check.limits.max_llm_calls = 12;
    check.blocks = vec![
        b(
            "history",
            "history",
            "Conversation",
            0.0,
            0.0,
            json!({ "max_messages": 10 }),
        ),
        b("message", "input", "Message", 0.0, 180.0, json!({})),
        b(
            "context",
            "combine",
            "Context",
            280.0,
            60.0,
            json!({ "inputs": 2, "format": CONTEXT_FORMAT }),
        ),
        b(
            "draft",
            "llm",
            "Draft",
            560.0,
            60.0,
            json!({
                "prompt": "_INPUT_\n\nWrite the assistant's reply.",
                "system": "You are a helpful assistant. Answer clearly and briefly.",
                "max_tokens": 1024
            }),
        ),
        b(
            "check",
            "llm",
            "Check",
            860.0,
            220.0,
            json!({
                "prompt": "Read this reply. Is it clear, correct and complete?\n\nReply:\n_INPUT_\n\nAnswer with just YES or NO.",
                "temperature": 0.0,
                "max_tokens": 8
            }),
        ),
        b(
            "decide",
            "branch",
            "Good enough?",
            1160.0,
            60.0,
            json!({ "pattern": "(?i)^\\W*yes" }),
        ),
        b(
            "improve",
            "llm",
            "Improve",
            1160.0,
            320.0,
            json!({
                "prompt": "Improve this reply. Fix anything unclear or wrong. Answer with only the improved reply.\n\n_INPUT_",
                "max_tokens": 1024
            }),
        ),
        b("reply", "output", "Reply", 1460.0, 60.0, json!({})),
    ];
    check.links = vec![
        l("history", "out", "context", "in1"),
        l("message", "out", "context", "in2"),
        l("context", "out", "draft", "in"),
        l("draft", "out", "decide", "value"),
        l("draft", "out", "check", "in"),
        l("check", "out", "decide", "test"),
        l("decide", "yes", "reply", "in"),
        l("decide", "no", "improve", "in"),
        l("improve", "out", "decide", "value"),
        l("improve", "out", "check", "in"),
    ];

    let plan = LoopFile::from_json(include_str!("../templates/five-step-plan.chummy.json"))
        .expect("the built-in plan template is a valid loop file");

    vec![
        Template {
            id: "blank",
            name: "Blank",
            description: "A Message block and a Reply block.",
            file: blank,
        },
        Template {
            id: "simple_chat",
            name: "Simple chat",
            description: "One model call with the conversation as context.",
            file: chat,
        },
        Template {
            id: "self_check",
            name: "Draft and self-check",
            description: "Drafts, checks its own answer, and improves it until the check passes.",
            file: check,
        },
        Template {
            id: "five_step_plan",
            name: "5 step recursive plan",
            description: "Plans your goal in 5 steps or fewer, does each step it can (with tools), and breaks down the rest, breadth-first.",
            file: plan,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::analyze;

    use crate::engine::{Host, LlmAnswer, LlmCall, RunInputs, RunStatus, ToolOutput, run};
    use crate::exchange::{Role, ToolCall, ToolInfo};
    use serde_json::{Value as Json, json};
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    /// Answers like a model that can do everything but writing a summary.
    #[derive(Default)]
    struct Planner {
        tools_run: Mutex<Vec<String>>,
    }

    impl Host for Planner {
        async fn llm(
            &self,
            call: LlmCall,
            _: &mut (dyn FnMut(&str) + Send),
        ) -> Result<LlmAnswer, String> {
            let last = call.messages.last().unwrap();
            let user = call
                .messages
                .iter()
                .rev()
                .find(|m| m.role == Role::User)
                .map(|m| m.text.to_lowercase())
                .unwrap_or_default();
            let text = |t: &str| {
                Ok(LlmAnswer {
                    text: t.into(),
                    ..Default::default()
                })
            };
            if last.role == Role::Tool {
                return text(&format!("Done: {}", last.text));
            }
            if !call.tools.is_empty() && user.contains("date") {
                return Ok(LlmAnswer {
                    calls: vec![ToolCall {
                        id: "t1".into(),
                        name: "current_time".into(),
                        arguments: json!({}),
                    }],
                    ..Default::default()
                });
            }
            if user.contains("yes or no") {
                return text(if user.contains("summary") {
                    "NO"
                } else {
                    "YES"
                });
            }
            if user.contains("too big") {
                return text("1. Gather notes\n2. Write two lines");
            }
            if user.contains("numbered") {
                return text("1. Find today's date\n2. Pick a topic\n3. Write a summary");
            }
            text("did it")
        }
        fn tool_info(&self, name: &str) -> Option<ToolInfo> {
            (name == "current_time").then(|| ToolInfo {
                name: name.into(),
                description: "Time.".into(),
                parameters: json!({ "type": "object", "properties": {} }),
            })
        }
        async fn run_tool(&self, name: &str, _: Json) -> Result<ToolOutput, String> {
            self.tools_run.lock().unwrap().push(name.into());
            Ok(ToolOutput {
                text: "Monday".into(),
                is_error: false,
            })
        }
        fn read_file(&self, _: &str) -> Result<String, String> {
            Err("no".into())
        }
        fn write_file(&self, _: &str, _: &str, _: bool) -> Result<(), String> {
            Err("no".into())
        }
        fn list_files(&self, _: &str) -> Result<Vec<String>, String> {
            Err("no".into())
        }
    }

    #[tokio::test]
    async fn the_plan_template_works_breadth_first() {
        let t = templates()
            .into_iter()
            .find(|t| t.id == "five_step_plan")
            .unwrap();
        let host = Planner::default();
        let inputs = RunInputs {
            message: "Write about today".into(),
            ..Default::default()
        };
        let out = run(&t.file, &host, &inputs, &AtomicBool::new(false), |_| {}).await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        let r = &out.replies;
        assert!(r[0].starts_with("**Plan**"), "{r:?}");
        assert_eq!(r[1], "**✓ Step: Find today's date**\n\nDone: Monday");
        assert_eq!(r[2], "**✓ Step: Pick a topic**\n\ndid it");
        assert!(
            r[3].starts_with("**Breaking down Step: Write a summary**"),
            "{r:?}"
        );
        // The sub-steps come after the top-level steps: breadth-first.
        assert_eq!(r[4], "**✓ Step: Gather notes**\n\ndid it");
        assert_eq!(r[5], "**✓ Step: Write two lines**\n\ndid it");
        assert_eq!(r.len(), 6);
        assert_eq!(*host.tools_run.lock().unwrap(), vec!["current_time"]);
        // Only the start exchange waits at the end, holding the running exchange.
        assert_eq!(out.waiting, vec!["pair"]);
    }

    #[test]
    fn templates_are_valid() {
        for t in templates() {
            let a = analyze(&t.file);
            assert!(a.ok, "{}: {:?}", t.id, a.issues);
            let warnings: Vec<_> = a
                .issues
                .iter()
                .filter(|i| t.id != "blank" || !i.message.contains("Reply"))
                .collect();
            assert!(
                t.id == "blank" || warnings.is_empty(),
                "{}: {:?}",
                t.id,
                warnings
            );
        }
    }
}
