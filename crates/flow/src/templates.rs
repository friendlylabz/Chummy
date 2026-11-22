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

    // Written by templates/make_templates.py.
    let plan = LoopFile::from_json(include_str!("../templates/five-step-plan.chummy.json"))
        .expect("the built-in plan template is a valid loop file");
    let plan_verify = LoopFile::from_json(include_str!(
        "../templates/five-step-plan-verify.chummy.json"
    ))
    .expect("the built-in plan verify template is a valid loop file");

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
            description: "Tries your goal straight away (with tools); anything too big is broken into 5 or fewer steps, each tried the same way, breadth-first.",
            file: plan,
        },
        Template {
            id: "five_step_plan_verify",
            name: "5 step recursive plan verify",
            description: "The 5 step recursive plan, plus a check of every result and plan: the model writes a yes/no question and answers it. Slower, more careful.",
            file: plan_verify,
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

    /// A scripted model. It can find the date (with a tool) and pick a topic;
    /// the goal and "write a summary" are too big. With `strict`, its checks
    /// reject the topic it picked and the first plan for the summary.
    #[derive(Default)]
    struct Planner {
        strict: bool,
        tools_run: Mutex<Vec<String>>,
        plan_checks_failed: Mutex<u32>,
        calls: Mutex<u32>,
    }

    impl Host for Planner {
        async fn llm(
            &self,
            call: LlmCall,
            _: &mut (dyn FnMut(&str) + Send),
        ) -> Result<LlmAnswer, String> {
            *self.calls.lock().unwrap() += 1;
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
            if !call.tools.is_empty() {
                // An attempt at "Task: …".
                return if user.contains("date") {
                    Ok(LlmAnswer {
                        calls: vec![ToolCall {
                            id: "t1".into(),
                            name: "current_time".into(),
                            arguments: json!({}),
                        }],
                        ..Default::default()
                    })
                } else if user.contains("write about today") || user.contains("summary") {
                    text("TOO BIG")
                } else {
                    text("did it")
                };
            }
            if user.contains("write one yes-or-no question") {
                return text("Does this answer make sense given the request?");
            }
            if user.contains("start with yes or no") {
                let request = user.lines().next().unwrap_or_default().to_string();
                let is_plan = user.contains("answer: 1.");
                if self.strict && request.contains("pick a topic") && !is_plan {
                    return text("NO, no topic was picked.");
                }
                if self.strict && request.contains("summary") && is_plan {
                    let mut n = self.plan_checks_failed.lock().unwrap();
                    *n += 1;
                    return text(if *n == 1 {
                        "NO, too vague."
                    } else {
                        "YES, fine."
                    });
                }
                return text("YES, it fits.");
            }
            if user.contains("better plan") {
                return text("1. Gather notes\n2. Write two lines");
            }
            if user.contains("too big to do in one go") {
                return text(if user.contains("write about today") {
                    "1. Find today's date\n2. Pick a topic\n3. Write a summary"
                } else {
                    "1. Gather notes\n2. Write two lines"
                });
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
        async fn ask(&self, _: crate::Ask) -> Result<crate::Answer, String> {
            Err("no".into())
        }
    }

    async fn plan_run(id: &str, host: &Planner, message: &str) -> crate::engine::RunOutcome {
        let t = templates().into_iter().find(|t| t.id == id).unwrap();
        let inputs = RunInputs {
            message: message.into(),
            ..Default::default()
        };
        run(&t.file, host, &inputs, &AtomicBool::new(false), |_| {}).await
    }

    #[tokio::test]
    async fn the_plan_template_tries_first_then_plans_breadth_first() {
        let host = Planner::default();
        let out = plan_run("five_step_plan", &host, "Write about today").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        let r = &out.replies;
        // The goal was tried first, found too big, then planned.
        assert_eq!(
            r[0],
            "**Breaking down Task: Write about today**\n\n1. Find today's date\n2. Pick a topic\n3. Write a summary"
        );
        assert_eq!(r[1], "**✓ Task: Find today's date**\n\nDone: Monday");
        assert_eq!(r[2], "**✓ Task: Pick a topic**\n\ndid it");
        assert!(
            r[3].starts_with("**Breaking down Task: Write a summary**"),
            "{r:?}"
        );
        // The sub-steps come after the top-level steps: breadth-first.
        assert_eq!(r[4], "**✓ Task: Gather notes**\n\ndid it");
        assert_eq!(r[5], "**✓ Task: Write two lines**\n\ndid it");
        assert_eq!(r.len(), 6);
        assert_eq!(*host.tools_run.lock().unwrap(), vec!["current_time"]);
        // Only the running exchange waits at the end.
        assert_eq!(out.waiting, vec!["pair"]);
    }

    #[tokio::test]
    async fn the_plan_template_one_shots_what_it_can() {
        let host = Planner::default();
        let out = plan_run("five_step_plan", &host, "What's the date?").await;
        assert_eq!(
            out.replies,
            vec!["**✓ Task: What's the date?**\n\nDone: Monday"]
        );
        // One call to use the tool, one to answer: nothing was planned.
        assert_eq!(*host.calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn the_verify_template_checks_results_and_plans() {
        let host = Planner {
            strict: true,
            ..Default::default()
        };
        let out = plan_run("five_step_plan_verify", &host, "Write about today").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        let r = &out.replies;
        assert!(
            r[0].starts_with("**Breaking down Task: Write about today**"),
            "{r:?}"
        );
        assert_eq!(r[1], "**✓ Task: Find today's date**\n\nDone: Monday");
        // "Pick a topic" was done, but failed its check, so it was broken down.
        assert_eq!(
            r[2],
            "**Breaking down Task: Pick a topic**\n\n1. Gather notes\n2. Write two lines"
        );
        // The summary's first plan failed its check and was made again.
        assert_eq!(
            r[3],
            "**Breaking down Task: Write a summary**\n\n1. Gather notes\n2. Write two lines"
        );
        // The second plan is used as it is (one retry, so it can't loop).
        assert_eq!(*host.plan_checks_failed.lock().unwrap(), 1);
        assert_eq!(r.len(), 8, "{r:?}");
        assert!(r[4..].iter().all(|m| m.starts_with("**✓ Task:")), "{r:?}");
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
