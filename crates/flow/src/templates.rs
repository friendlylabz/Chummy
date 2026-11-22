//! Starting points offered when creating a loop. Nothing is installed until
//! the user picks one.

use serde::Serialize;
use serde_json::json;

use crate::format::{Block, Link, LoopFile, Position};

/// The id of the template new agents get when nothing is configured.
pub const ASSISTANT: &str = "assistant";

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
        comment: String::new(),
    }
}

fn l(a: &str, ap: &str, c: &str, cp: &str) -> Link {
    Link::new((a, ap), (c, cp))
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
    let assistant = LoopFile::from_json(include_str!("../templates/assistant.chummy.json"))
        .expect("the built-in assistant template is a valid loop file");

    vec![
        Template {
            id: ASSISTANT,
            name: "Chummy assistant",
            description: "The default: asks if your request is unclear, tries it with tools (each call checked first), breaks anything too big into steps, checks every result and plan, keeps working notes, and carries on past failures.",
            file: assistant,
        },
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
        /// Requests mentioning "it" are unclear.
        picky: bool,
        /// Tool-call checks say no.
        decline: bool,
        /// Model calls whose prompt contains any of these fail.
        fail_on: Vec<&'static str>,
        /// Answers the user gives.
        answers: Mutex<Vec<String>>,
        asked: Mutex<Vec<String>>,
        /// Every system message sent, to see the notes.
        systems: Mutex<Vec<String>>,
        /// The last message of every call.
        lasts: Mutex<Vec<String>>,
        notes_updates: Mutex<u32>,
    }

    impl Host for Planner {
        async fn llm(
            &self,
            call: LlmCall,
            _: &mut (dyn FnMut(&str) + Send),
        ) -> Result<LlmAnswer, String> {
            *self.calls.lock().unwrap() += 1;
            if let Some(m) = call.messages.last() {
                self.lasts.lock().unwrap().push(m.text.clone());
            }
            if let Some(sys) = call.messages.iter().find(|m| m.role == Role::System) {
                self.systems.lock().unwrap().push(sys.text.clone());
            }
            let all = call
                .messages
                .iter()
                .map(|m| m.text.to_lowercase())
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(f) = self.fail_on.iter().find(|f| all.contains(*f)) {
                return Err(format!("the model server failed on “{f}”"));
            }
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
            if user.contains("clear enough to act on") {
                return text(if self.picky && user.contains("about it") {
                    "Which topic do you mean?"
                } else {
                    "CLEAR"
                });
            }
            if user.contains("sensible and safe") {
                return text(if self.decline {
                    "NO, the date isn't needed."
                } else {
                    "YES, it fits the task."
                });
            }
            if user.contains("rewrite the working notes") {
                let mut n = self.notes_updates.lock().unwrap();
                *n += 1;
                return text(&format!("- update {n}"));
            }
            if last.role == Role::Tool {
                return text(&if last.text.starts_with("Not run") {
                    "I'll do without the tool: Sunday, probably.".to_string()
                } else {
                    format!("Done: {}", last.text)
                });
            }
            if !call.tools.is_empty()
                && user.contains("ask me")
                && last.role == Role::User
                && user.starts_with("task:")
            {
                return text("QUESTION: Which colour?");
            }
            if !call.tools.is_empty() {
                // An attempt at "Task: …".
                return if user.contains("date") {
                    Ok(LlmAnswer {
                        calls: vec![ToolCall {
                            id: "t1".into(),
                            name: "current_time".into(),
                            arguments: json!({}),
                            online: false,
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
        async fn run_tool(&self, name: &str, _: Json, _online: bool) -> Result<ToolOutput, String> {
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
        async fn ask(&self, a: crate::Ask) -> Result<crate::Answer, String> {
            self.asked.lock().unwrap().push(a.text);
            self.answers
                .lock()
                .unwrap()
                .pop()
                .map(crate::Answer::Text)
                .ok_or_else(|| "nobody to ask".into())
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

    #[tokio::test]
    async fn the_assistant_one_shots_with_a_checked_tool_call() {
        let host = Planner::default();
        let out = plan_run(ASSISTANT, &host, "What's the date?").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(
            out.replies,
            ["**✓ Task: What's the date?**\n\nDone: Monday"]
        );
        assert_eq!(*host.tools_run.lock().unwrap(), ["current_time"]);
        // The notes were rewritten after the result.
        assert_eq!(*host.notes_updates.lock().unwrap(), 1);
        assert!(
            host.asked.lock().unwrap().is_empty(),
            "a clear request asks nothing"
        );
    }

    #[tokio::test]
    async fn the_assistant_plans_and_keeps_notes() {
        let host = Planner::default();
        let out = plan_run(ASSISTANT, &host, "Write about today").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        let r = &out.replies;
        // Every breakdown shows the whole plan, nested, with finished tasks
        // ticked.
        assert_eq!(
            r[0],
            "**▾ Too big for one go, so I broke it down.** The whole plan now:\n\
             ▾ 1. Write about today\n    ○ 1.1. Find today's date\n    ○ 1.2. Pick a topic\n    ○ 1.3. Write a summary"
        );
        assert_eq!(r[1], "**✓ Task: Find today's date**\n\nDone: Monday");
        let second = r
            .iter()
            .find(|m| m.contains("▾ 1.3. Write a summary"))
            .unwrap();
        assert!(
            second.ends_with(
                "✓ 1.1. Find today's date\n    ✓ 1.2. Pick a topic\n    ▾ 1.3. Write a summary\n        \
                 ○ 1.3.1. Gather notes\n        ○ 1.3.2. Write two lines"
            ),
            "{second}"
        );
        // The model sees the plan too.
        assert!(
            host.systems
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.contains("○ 1.3. Write a summary"))
        );
        // Notes: written after every result and plan, and seen afterwards.
        let updates = *host.notes_updates.lock().unwrap() as usize;
        assert_eq!(updates, r.len());
        let systems = host.systems.lock().unwrap();
        assert!(systems.iter().any(|s| s.contains("(nothing yet)")));
        assert!(
            systems
                .iter()
                .any(|s| s.contains(&format!("- update {}", updates - 1)))
        );
    }

    #[tokio::test]
    async fn the_assistant_asks_when_unclear_or_stuck() {
        // Unclear: one question first, and the answer joins the request.
        let host = Planner {
            picky: true,
            ..Default::default()
        };
        host.answers.lock().unwrap().push("Gardening".into());
        let out = plan_run(ASSISTANT, &host, "Write a line about it").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(*host.asked.lock().unwrap(), ["Which topic do you mean?"]);
        // The heading shows the request's first line; the model sees the
        // answer too.
        assert_eq!(
            out.replies[0],
            "**✓ Task: Write a line about it**\n\ndid it"
        );
        assert!(
            host.lasts
                .lock()
                .unwrap()
                .iter()
                .any(|m| m.contains("The user answered: Gardening"))
        );

        // Stuck: the model asks a QUESTION mid-task and goes on with the answer.
        let host = Planner::default();
        host.answers.lock().unwrap().push("Blue".into());
        let out = plan_run(ASSISTANT, &host, "Paint it, ask me first").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(*host.asked.lock().unwrap(), ["QUESTION: Which colour?"]);
        assert_eq!(
            out.replies,
            ["**✓ Task: Paint it, ask me first**\n\ndid it"]
        );

        // Nobody to ask: it carries on with its best judgement.
        let host = Planner::default();
        let out = plan_run(ASSISTANT, &host, "Paint it, ask me first").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(out.replies.len(), 1);
    }

    #[tokio::test]
    async fn the_assistant_declines_bad_tool_calls() {
        let host = Planner {
            decline: true,
            ..Default::default()
        };
        let out = plan_run(ASSISTANT, &host, "What's the date?").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert!(
            host.tools_run.lock().unwrap().is_empty(),
            "the declined call never ran"
        );
        assert_eq!(
            out.replies,
            ["**✓ Task: What's the date?**\n\nI'll do without the tool: Sunday, probably."]
        );
    }

    #[tokio::test]
    async fn the_assistant_carries_on_past_failures() {
        // Clarifying, checking calls, verifying and updating notes all fail:
        // the work still gets done.
        let host = Planner {
            fail_on: vec![
                "clear enough",
                "sensible and safe",
                "yes-or-no question",
                "start with yes",
                "rewrite the working notes",
            ],
            ..Default::default()
        };
        let out = plan_run(ASSISTANT, &host, "What's the date?").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(
            out.replies,
            ["**✓ Task: What's the date?**\n\nDone: Monday"]
        );
        // The attempt itself fails: a short message instead of a stopped run.
        let host = Planner {
            fail_on: vec!["do the user's latest task"],
            ..Default::default()
        };
        let out = plan_run(ASSISTANT, &host, "What's the date?").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(out.replies.len(), 1);
        assert!(
            out.replies[0].contains("I couldn't finish this task"),
            "{:?}",
            out.replies
        );
        // Planning fails too: said once, no steps.
        let host = Planner {
            fail_on: vec!["too big to do in one go"],
            ..Default::default()
        };
        let out = plan_run(ASSISTANT, &host, "Write about today").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert!(
            out.replies[0].contains("couldn't break this task down"),
            "{:?}",
            out.replies
        );
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
