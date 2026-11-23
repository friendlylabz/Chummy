//! Starting points offered when creating a loop. Nothing is installed until
//! the user picks one.

use serde::Serialize;
use serde_json::json;

use crate::format::{Block, Link, LoopFile, Position};

/// The id of the template new agents get when nothing is configured.
pub const ASSISTANT: &str = "assistant";

/// Chummy assistant 2: one agent loop (model ⇄ tools until it answers), with
/// its own to-do list. A candidate for the default.
pub const ASSISTANT2: &str = "assistant2";

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
                "tools": "off"
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
                "tools": "off"
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
                "tools": "off"
            }),
        ),
        b(
            "decide",
            "branch",
            "Good enough?",
            1160.0,
            60.0,
            json!({ "pattern": "(?i)^\\W*yes|^\\s*$" }),
        ),
        b(
            "improve",
            "llm",
            "Improve",
            1160.0,
            320.0,
            json!({
                "prompt": "Improve this reply. Fix anything unclear or wrong. Answer with only the improved reply.\n\n_INPUT_",
                "tools": "off"
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
    // Written by templates/make_templates.py from templates/assistant2.py.
    let assistant2 = LoopFile::from_json(include_str!("../templates/assistant2.chummy.json"))
        .expect("the built-in assistant 2 template is a valid loop file");

    vec![
        Template {
            id: ASSISTANT,
            name: "Chummy assistant",
            description: "The default: asks if your request is unclear, does it in one go if it can, otherwise plans everything first (checked, specific steps), then does each step with up to 5 checked tries. Every tool call is checked, and it keeps working notes. Stops with a reason if a step can't be done.",
            file: assistant,
        },
        Template {
            id: ASSISTANT2,
            name: "Chummy assistant 2",
            description: "One agent loop, like a coding agent's: the model works with tools, call after call, until it's done. It says what it's doing as it goes, keeps its own to-do list for bigger jobs, reads CHUMMY.md from the folder for your instructions, and summarises long conversations. Sent back to finish if it stops with steps unchecked.",
            file: assistant2,
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
        /// Judges and checks answer with nothing, like a thinking model whose
        /// reply was all thinking.
        blank: bool,
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
            if self.blank
                && [
                    "clear enough to act on",
                    "sensible and safe",
                    "start with yes or no",
                ]
                .iter()
                .any(|j| user.contains(j))
            {
                return text("");
            }
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
                            network: crate::blocks::Switch::Off,
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
        fn tool_names(&self) -> Vec<String> {
            vec!["current_time".into()]
        }
        fn tool_info(&self, name: &str) -> Option<ToolInfo> {
            (name == "current_time").then(|| ToolInfo {
                name: name.into(),
                description: "Time.".into(),
                parameters: json!({ "type": "object", "properties": {} }),
            })
        }
        async fn run_tool(
            &self,
            name: &str,
            _: Json,
            _online: crate::blocks::Switch,
        ) -> Result<ToolOutput, String> {
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

    #[test]
    fn only_the_working_blocks_get_tools() {
        use crate::blocks::{BlockKind, Switch};
        for t in templates() {
            let mut blocks: Vec<&Block> = t.file.blocks.iter().collect();
            for f in &t.file.functions {
                blocks.extend(f.blocks.iter());
            }
            for blk in blocks.into_iter().filter(|b| b.kind == "llm") {
                let Ok(BlockKind::Llm(p)) = BlockKind::parse(&blk.kind, &blk.params) else {
                    panic!("{}", blk.id)
                };
                assert_eq!(
                    p.max_tokens, None,
                    "{} {} inherits the model's",
                    t.id, blk.id
                );
                if blk.id == "work" {
                    assert!(
                        p.tools.is_inherit() && p.network == Switch::Inherit,
                        "{}",
                        t.id
                    );
                } else {
                    assert_eq!(p.tools.all, Switch::Off, "{} {}", t.id, blk.id);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // The Chummy assistant: one go, or plan fully first, then carry out.

    /// A scripted model for the assistant. It recognises each kind of call by
    /// its prompt and answers as set up.
    #[derive(Default)]
    struct Scripted {
        /// The one-go judge says ONE GO.
        one_go: bool,
        /// The plan for the goal (numbered lines).
        plan: &'static str,
        /// Steps the step judge says are too big, with their own plans.
        big: Vec<(&'static str, &'static str)>,
        /// The first plan check says NO (too vague); the plan written again is this.
        vague_first: Option<&'static str>,
        /// Results for anything containing the key fail their check this many
        /// times (u32::MAX: always).
        failing: Mutex<Vec<(&'static str, u32)>>,
        /// Tool-call checks say no.
        decline: bool,
        /// Judges and checks answer with nothing.
        blank: bool,
        /// Requests mentioning "about it" are unclear.
        picky: bool,
        /// Model calls whose messages contain any of these fail.
        fail_on: Vec<&'static str>,
        answers: Mutex<Vec<String>>,
        asked: Mutex<Vec<String>>,
        tools_run: Mutex<Vec<String>>,
        /// Each kind of call, in order: clarify, one_go, plan, plan_check,
        /// judge, work, tool_check, question, check, notes, explain, final.
        kinds: Mutex<Vec<&'static str>>,
        /// The brief of every work call.
        briefs: Mutex<Vec<String>>,
        /// Every system message sent to a work call.
        systems: Mutex<Vec<String>>,
        notes_updates: Mutex<u32>,
    }

    /// The text after `marker` on its line, in the original case.
    fn after(text: &str, marker: &str) -> String {
        let i = text.to_lowercase().find(marker).expect(marker) + marker.len();
        text[i..]
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    impl Scripted {
        fn kinds(&self) -> Vec<&'static str> {
            self.kinds.lock().unwrap().clone()
        }
        /// The step each first try worked on, in order.
        fn worked(&self) -> Vec<String> {
            let mut out: Vec<String> = Vec::new();
            for b in self.briefs.lock().unwrap().iter() {
                let lower = b.to_lowercase();
                let marker = if lower.contains("your step now: ") {
                    "your step now: "
                } else {
                    "your task now: "
                };
                if !b.contains("didn't pass") {
                    let s = after(b, marker);
                    if out.last() != Some(&s) {
                        out.push(s);
                    }
                }
            }
            out
        }
        fn fails(&self, request: &str) -> bool {
            let mut f = self.failing.lock().unwrap();
            for (key, n) in f.iter_mut() {
                if request.contains(&key.to_lowercase()) && *n > 0 {
                    *n = n.saturating_sub(if *n == u32::MAX { 0 } else { 1 });
                    return true;
                }
            }
            false
        }
    }

    impl Host for Scripted {
        async fn llm(
            &self,
            call: LlmCall,
            _: &mut (dyn FnMut(&str) + Send),
        ) -> Result<LlmAnswer, String> {
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
            let user_msg = call
                .messages
                .iter()
                .rev()
                .find(|m| m.role == Role::User)
                .map(|m| m.text.clone())
                .unwrap_or_default();
            let user = user_msg.to_lowercase();
            let text = |t: &str| {
                Ok(LlmAnswer {
                    text: t.into(),
                    ..Default::default()
                })
            };
            let kind = |k: &'static str| self.kinds.lock().unwrap().push(k);
            let blank = |t: &str| if self.blank { "" } else { t }.to_string();

            if user.contains("clear enough to act on") {
                kind("clarify");
                let q = self.picky && user.contains("about it");
                return text(&blank(if q {
                    "Which topic do you mean?"
                } else {
                    "CLEAR"
                }));
            }
            if user.contains("whole goal in one go") {
                kind("one_go");
                return text(&blank(if self.one_go {
                    "ONE GO: it's simple."
                } else {
                    "PLAN: it has parts."
                }));
            }
            if user.contains("check this plan.") {
                kind("plan_check");
                let vague = self.vague_first.is_some() && !self.kinds().contains(&"replan");
                return text(&blank(if vague {
                    "NO, step 1 is too vague."
                } else {
                    "YES, it's concrete."
                }));
            }
            if user.contains("write a plan: the steps to do") {
                if user.contains("a first plan was") {
                    kind("replan");
                    return text(self.vague_first.unwrap());
                }
                kind("plan");
                if user.contains("plan it in smaller steps: ") {
                    let step = after(&user_msg, "plan it in smaller steps: ");
                    let sub = self
                        .big
                        .iter()
                        .find(|(s, _)| *s == step)
                        .expect("planned a small step");
                    return text(sub.1);
                }
                return text(self.plan);
            }
            if user.contains("one step of it: ") {
                kind("judge");
                let step = after(&user_msg, "one step of it: ");
                let big = self.big.iter().any(|(s, _)| *s == step);
                return text(&blank(if big {
                    "BREAK DOWN: several parts."
                } else {
                    "ONE GO"
                }));
            }
            if user.contains("sensible and safe") {
                kind("tool_check");
                return text(&blank(if self.decline {
                    "NO, the date isn't needed."
                } else {
                    "YES, it fits."
                }));
            }
            if user.contains("rewrite the working notes") {
                kind("notes");
                let mut n = self.notes_updates.lock().unwrap();
                *n += 1;
                return text(&format!("- update {n}"));
            }
            if user.contains("tried 5 times") {
                kind("explain");
                return text("The report needs figures I couldn't find.");
            }
            if user.contains("write the final reply to the user") {
                kind("final");
                return text("Everything is done.");
            }
            if user.contains("write one yes-or-no question") {
                kind("question");
                return text(&blank("Does this answer do what was asked?"));
            }
            if user.contains("start with yes or no") {
                kind("check");
                let request = user.split("\n\nanswer:").next().unwrap_or_default();
                return text(&blank(if self.fails(request) {
                    "NO, it's wrong."
                } else {
                    "YES, it fits."
                }));
            }
            assert!(!call.tools.is_empty(), "an unexpected call: {user}");
            kind("work");
            if let Some(sys) = call.messages.iter().find(|m| m.role == Role::System) {
                self.systems.lock().unwrap().push(sys.text.clone());
            }
            if last.role == Role::Tool {
                return text(&if last.text.starts_with("Not run") {
                    "I'll do without the tool: Sunday, probably.".to_string()
                } else {
                    format!("Done: {}", last.text)
                });
            }
            self.briefs.lock().unwrap().push(user_msg.clone());
            let marker = if user.contains("your step now: ") {
                "your step now: "
            } else {
                "your task now: "
            };
            let step = after(&user_msg, marker);
            if step.to_lowercase().contains("date") {
                return Ok(LlmAnswer {
                    calls: vec![ToolCall {
                        id: "t1".into(),
                        name: "current_time".into(),
                        arguments: json!({}),
                        network: crate::blocks::Switch::Off,
                    }],
                    ..Default::default()
                });
            }
            text(&format!("Did: {step}"))
        }
        fn tool_names(&self) -> Vec<String> {
            vec!["current_time".into()]
        }
        fn tool_info(&self, name: &str) -> Option<ToolInfo> {
            (name == "current_time").then(|| ToolInfo {
                name: name.into(),
                description: "Time.".into(),
                parameters: json!({ "type": "object", "properties": {} }),
            })
        }
        async fn run_tool(
            &self,
            name: &str,
            _: Json,
            _: crate::blocks::Switch,
        ) -> Result<ToolOutput, String> {
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

    async fn assist(host: &Scripted, message: &str) -> crate::engine::RunOutcome {
        let t = templates().into_iter().find(|t| t.id == ASSISTANT).unwrap();
        let inputs = RunInputs {
            message: message.into(),
            ..Default::default()
        };
        run(&t.file, host, &inputs, &AtomicBool::new(false), |_| {}).await
    }

    const REPORT_PLAN: &str = "1. Collect the facts\n2. Draft the report\n3. Send the summary";

    #[tokio::test]
    async fn one_go_with_a_checked_tool_call() {
        let host = Scripted {
            one_go: true,
            ..Default::default()
        };
        let out = assist(&host, "What's the date?").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(out.replies, ["Done: Monday"]);
        assert_eq!(*host.tools_run.lock().unwrap(), ["current_time"]);
        let kinds = host.kinds();
        assert!(kinds.contains(&"tool_check") && kinds.contains(&"check"));
        assert!(!kinds.contains(&"plan"), "nothing was planned: {kinds:?}");
    }

    #[tokio::test]
    async fn plans_fully_first_then_carries_out_depth_first() {
        let host = Scripted {
            plan: REPORT_PLAN,
            big: vec![(
                "Collect the facts",
                "1. List the sources\n2. Note three facts",
            )],
            ..Default::default()
        };
        let out = assist(&host, "Write a report").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        // Big steps are planned in place (depth first), then the steps are done in that order.
        assert_eq!(
            host.worked(),
            [
                "List the sources",
                "Note three facts",
                "Draft the report",
                "Send the summary"
            ]
        );
        // Nothing is done while planning.
        let kinds = host.kinds();
        let last_planning = kinds
            .iter()
            .rposition(|k| matches!(*k, "plan" | "plan_check" | "judge"))
            .unwrap();
        let first_work = kinds.iter().position(|k| *k == "work").unwrap();
        assert!(last_planning < first_work, "{kinds:?}");
        // Only top-level steps are judged: two levels at most.
        assert_eq!(kinds.iter().filter(|k| **k == "judge").count(), 3);
        // The whole plan is shown, nested, before any result.
        assert!(
            out.replies[0].starts_with("**▾ Here's the plan:**"),
            "{:?}",
            out.replies
        );
        assert!(
            out.replies[0].contains("○ 1.1.1. List the sources"),
            "{}",
            out.replies[0]
        );
        assert_eq!(
            out.replies[1],
            "**✓ List the sources**\n\nDid: List the sources"
        );
        assert_eq!(
            out.replies.len(),
            6,
            "plan, four results, final: {:?}",
            out.replies
        );
        assert_eq!(out.replies[5], "**Done.** Everything is done.");
        // Notes: after the plan and after each step; every step sees them, and the results so far.
        assert_eq!(*host.notes_updates.lock().unwrap(), 5);
        let systems = host.systems.lock().unwrap();
        assert!(
            systems[0].contains("Your working notes:\n- update 1"),
            "{}",
            systems[0]
        );
        assert!(
            systems[0]
                .contains("    ▾ 1.1. Collect the facts\n        ▸ 1.1.1. List the sources\n")
        );
        assert!(
            systems
                .last()
                .unwrap()
                .contains("✓ Draft the report\nDid: Draft the report")
        );
    }

    #[tokio::test]
    async fn vague_plans_are_written_again() {
        let host = Scripted {
            plan: "1. Do the work",
            vague_first: Some("1. Write the intro to intro.md\n2. Write the ending to end.md"),
            ..Default::default()
        };
        let out = assist(&host, "Write a story").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(
            host.worked(),
            ["Write the intro to intro.md", "Write the ending to end.md"]
        );
    }

    #[tokio::test]
    async fn one_go_falls_back_to_a_plan() {
        let host = Scripted {
            one_go: true,
            plan: "1. Pick a topic\n2. Write four lines",
            failing: Mutex::new(vec![("write a poem", 2)]),
            ..Default::default()
        };
        let out = assist(&host, "Write a poem").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(
            host.worked(),
            ["Write a poem", "Pick a topic", "Write four lines"]
        );
        assert!(out.replies[0].starts_with("↻ That didn't work in one go"));
        assert!(*host.notes_updates.lock().unwrap() >= 1);
    }

    #[tokio::test]
    async fn steps_get_five_tries_then_the_run_stops_with_a_reason() {
        // Passes on the third try.
        let host = Scripted {
            plan: REPORT_PLAN,
            failing: Mutex::new(vec![("draft the report", 2)]),
            ..Default::default()
        };
        let out = assist(&host, "Write a report").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        let briefs = host.briefs.lock().unwrap().clone();
        let drafts: Vec<_> = briefs
            .iter()
            .filter(|b| b.contains("Draft the report"))
            .collect();
        assert_eq!(drafts.len(), 3);
        assert!(drafts[2].contains("Try 1 didn't pass: NO, it's wrong.\nFix that this time."));
        assert!(drafts[2].contains("Try 2 didn't pass"));

        // Never passes: five tries, an explanation, and the run stops there.
        let host = Scripted {
            plan: REPORT_PLAN,
            failing: Mutex::new(vec![("draft the report", u32::MAX)]),
            ..Default::default()
        };
        let out = assist(&host, "Write a report").await;
        let briefs = host.briefs.lock().unwrap();
        assert_eq!(
            briefs
                .iter()
                .filter(|b| b.contains("Draft the report"))
                .count(),
            5
        );
        assert!(
            !briefs.iter().any(|b| b.contains("Send the summary")),
            "later steps don't run"
        );
        assert!(host.kinds().contains(&"explain"));
        let last = out.replies.last().unwrap();
        assert!(
            last.starts_with("**✗ I couldn't do this step: Draft the report**"),
            "{last}"
        );
        assert!(last.contains("figures I couldn't find"));
        assert!(
            matches!(&out.status, RunStatus::Failed { error, .. }
                if error.contains("Couldn't do the step “Draft the report”") && error.contains("figures")),
            "{:?}",
            out.status
        );
    }

    #[tokio::test]
    async fn blank_judges_never_ask_or_block() {
        // A thinking model's judge reply can be empty once its thinking is
        // removed: that counts as clear, one go, approved and passed.
        let host = Scripted {
            blank: true,
            picky: true,
            ..Default::default()
        };
        let out = assist(&host, "What's the date about it?").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert!(host.asked.lock().unwrap().is_empty(), "no empty question");
        assert_eq!(*host.tools_run.lock().unwrap(), ["current_time"]);
        assert_eq!(out.replies, ["Done: Monday"]);
    }

    #[tokio::test]
    async fn unclear_requests_get_one_question() {
        let host = Scripted {
            one_go: true,
            picky: true,
            answers: Mutex::new(vec!["Cats".into()]),
            ..Default::default()
        };
        let out = assist(&host, "Write about it").await;
        assert_eq!(*host.asked.lock().unwrap(), ["Which topic do you mean?"]);
        assert!(host.briefs.lock().unwrap()[0].contains("The user answered: Cats"));
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
    }

    #[tokio::test]
    async fn bad_tool_calls_are_declined() {
        let host = Scripted {
            one_go: true,
            decline: true,
            ..Default::default()
        };
        let out = assist(&host, "What's the date?").await;
        assert!(host.tools_run.lock().unwrap().is_empty());
        assert_eq!(out.replies, ["I'll do without the tool: Sunday, probably."]);
    }

    #[tokio::test]
    async fn failed_planning_does_the_goal_as_one_step() {
        let host = Scripted {
            fail_on: vec!["write a plan: the steps"],
            ..Default::default()
        };
        let out = assist(&host, "Write a report").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(host.worked(), ["Write a report"]);
        assert_eq!(
            out.replies,
            [
                "▾ I couldn't break this into steps, so I'll do it as one.",
                "**✓ Write a report**\n\nDid: Write a report"
            ]
        );
        // The goal is ticked in the plan outline.
        assert!(host.systems.lock().unwrap()[0].contains("▸ 1. Write a report"));
    }

    #[tokio::test]
    async fn failed_notes_dont_stop_the_work() {
        let host = Scripted {
            plan: REPORT_PLAN,
            fail_on: vec!["rewrite the working notes"],
            ..Default::default()
        };
        let out = assist(&host, "Write a report").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(host.worked().len(), 3);
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

    // ------------------------------------------------------------------
    // Chummy assistant 2: one agent loop with the model's own to-do list.

    /// A scripted model for assistant 2. Work calls (the ones with the
    /// "You are Chummy" system message) take the next scripted answer;
    /// summaries answer "SUMMARY".
    #[derive(Default)]
    struct Agentic {
        script: Mutex<std::collections::VecDeque<LlmAnswer>>,
        /// Every work call, in order.
        work: Mutex<Vec<LlmCall>>,
        summaries: Mutex<u32>,
        /// What the echo tool returns.
        tool_text: String,
        tools_run: Mutex<Vec<Json>>,
        files: std::collections::HashMap<String, String>,
        /// Model calls whose messages contain this fail.
        fail_on: Option<&'static str>,
    }

    fn says(t: &str) -> LlmAnswer {
        LlmAnswer {
            text: t.into(),
            ..Default::default()
        }
    }

    fn echo(t: &str, n: u32) -> LlmAnswer {
        LlmAnswer {
            text: t.into(),
            calls: vec![ToolCall {
                id: format!("e{n}"),
                name: "echo".into(),
                arguments: json!({ "text": format!("call {n}") }),
                network: crate::blocks::Switch::Inherit,
            }],
            ..Default::default()
        }
    }

    impl Agentic {
        fn with(script: Vec<LlmAnswer>) -> Self {
            Agentic {
                script: Mutex::new(script.into()),
                tool_text: "hi".into(),
                ..Default::default()
            }
        }
        fn system(&self, i: usize) -> String {
            self.work.lock().unwrap()[i].messages[0].text.clone()
        }
        fn calls(&self) -> usize {
            self.work.lock().unwrap().len()
        }
    }

    impl Host for Agentic {
        async fn llm(
            &self,
            call: LlmCall,
            _: &mut (dyn FnMut(&str) + Send),
        ) -> Result<LlmAnswer, String> {
            let all = call
                .messages
                .iter()
                .map(|m| m.text.to_lowercase())
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(f) = self.fail_on.filter(|f| all.contains(f)) {
                return Err(format!("the model server failed on “{f}”"));
            }
            let last = call.messages.last().unwrap().text.to_lowercase();
            if last.contains("write a summary to carry on from") {
                assert!(call.tools.is_empty(), "summaries get no tools");
                *self.summaries.lock().unwrap() += 1;
                return Ok(says("SUMMARY of the work"));
            }
            assert!(
                call.messages[0].role == Role::System
                    && call.messages[0].text.starts_with("You are Chummy"),
                "an unexpected call: {call:?}"
            );
            assert!(!call.tools.is_empty(), "the work block gets the tools");
            self.work.lock().unwrap().push(call);
            Ok(self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| says("(script ran out)")))
        }
        fn tool_names(&self) -> Vec<String> {
            vec!["echo".into()]
        }
        fn tool_info(&self, name: &str) -> Option<ToolInfo> {
            (name == "echo").then(|| ToolInfo {
                name: name.into(),
                description: "Echoes.".into(),
                parameters: json!({ "type": "object", "properties": { "text": { "type": "string" } } }),
            })
        }
        fn agent_allows(&self, _: &str) -> bool {
            true
        }
        async fn run_tool(
            &self,
            _: &str,
            args: Json,
            _: crate::blocks::Switch,
        ) -> Result<ToolOutput, String> {
            self.tools_run.lock().unwrap().push(args);
            Ok(ToolOutput {
                text: self.tool_text.clone(),
                is_error: false,
            })
        }
        fn read_file(&self, path: &str) -> Result<String, String> {
            self.files
                .get(path)
                .cloned()
                .ok_or_else(|| format!("there's no file “{path}”"))
        }
        fn write_file(&self, _: &str, _: &str, _: bool) -> Result<(), String> {
            Err("no".into())
        }
        fn list_files(&self, path: &str) -> Result<Vec<String>, String> {
            assert_eq!(path, ".");
            Ok(vec!["docs/".into(), "notes.txt".into()])
        }
        async fn ask(&self, _: crate::Ask) -> Result<crate::Answer, String> {
            Err("nobody to ask".into())
        }
    }

    async fn assist2(host: &Agentic, message: &str) -> crate::engine::RunOutcome {
        let t = templates()
            .into_iter()
            .find(|t| t.id == ASSISTANT2)
            .unwrap();
        let inputs = RunInputs {
            message: message.into(),
            history: vec![
                crate::engine::ChatTurn {
                    role: "user".into(),
                    text: "Hello".into(),
                },
                crate::engine::ChatTurn {
                    role: "assistant".into(),
                    text: "Hi there.".into(),
                },
            ],
            ..Default::default()
        };
        run(&t.file, host, &inputs, &AtomicBool::new(false), |_| {}).await
    }

    #[tokio::test]
    async fn assistant2_answers_straight_away_when_no_tools_are_needed() {
        let host = Agentic::with(vec![says("Paris.")]);
        let out = assist2(&host, "What's the capital of France?").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(out.replies, ["Paris."]);
        assert_eq!(out.llm_calls, 1);
        let sys = host.system(0);
        assert!(sys.contains("# Your to-do list now\n(none yet)"), "{sys}");
        assert!(sys.contains("docs/\nnotes.txt"), "{sys}");
        assert!(
            sys.contains("(none: this folder has no CHUMMY.md)"),
            "{sys}"
        );
        assert!(sys.contains("User: Hello\n\nAssistant: Hi there."), "{sys}");
        // Only the request goes in the exchange; the rest is the system message.
        let first = &host.work.lock().unwrap()[0].messages;
        assert_eq!(first.len(), 2);
        assert_eq!(first[1].text, "What's the capital of France?");
    }

    #[tokio::test]
    async fn assistant2_reads_chummy_md() {
        let mut host = Agentic::with(vec![says("Bonjour.")]);
        host.files
            .insert("CHUMMY.md".into(), "Always answer in French.".into());
        let out = assist2(&host, "Hi").await;
        assert_eq!(out.replies, ["Bonjour."]);
        assert!(
            host.system(0)
                .contains("(CHUMMY.md)\nAlways answer in French.")
        );
    }

    #[tokio::test]
    async fn assistant2_loops_through_tools_and_says_what_its_doing() {
        let host = Agentic::with(vec![
            echo("I'll look it up.", 1),
            echo("", 2),
            says("It says hi, twice."),
        ]);
        let out = assist2(&host, "What does echo say?").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        // The call-only turn shows nothing (older text isn't repeated).
        assert_eq!(out.replies, ["I'll look it up.", "It says hi, twice."]);
        assert_eq!(host.tools_run.lock().unwrap().len(), 2);
        // Each call continues the same exchange, with the results in it.
        let work = host.work.lock().unwrap();
        assert_eq!(work.len(), 3);
        let third = &work[2].messages;
        assert_eq!(third.iter().filter(|m| m.role == Role::Tool).count(), 2);
        assert_eq!(third[2].text, "I'll look it up.");
    }

    #[tokio::test]
    async fn assistant2_keeps_the_models_own_todo_list() {
        let host = Agentic::with(vec![
            echo("I'll do it in two steps.\n\n- [ ] Read a\n- [ ] Write b", 1),
            echo("- [x] Read a\n- [ ] Write b", 2),
            says("- [x] Read a\n- [x] Write b\n\nAll done."),
        ]);
        let out = assist2(&host, "Copy a to b").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(host.calls(), 3, "nothing was sent back");
        assert!(
            host.system(1)
                .contains("now\n- [ ] Read a\n- [ ] Write b\n")
        );
        assert!(
            host.system(2)
                .contains("now\n- [x] Read a\n- [ ] Write b\n")
        );
        assert_eq!(out.replies.len(), 3);
    }

    #[tokio::test]
    async fn assistant2_sends_it_back_when_steps_are_unchecked() {
        let host = Agentic::with(vec![
            echo("- [ ] Read a\n- [ ] Write b", 1),
            says("I've read a."),
            echo("", 2),
            says("- [x] Read a\n- [x] Write b\n\nAll done."),
        ]);
        let out = assist2(&host, "Copy a to b").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(host.calls(), 4);
        // The early stop is shown, then the model is told to carry on, in a
        // fresh exchange holding the request, the work so far and the list.
        assert_eq!(out.replies[1], "I've read a.");
        let work = host.work.lock().unwrap();
        let back = &work[2].messages;
        assert_eq!(back.len(), 2, "{back:?}");
        let msg = &back[1].text;
        assert!(msg.contains("The user's request:\nCopy a to b"), "{msg}");
        assert!(msg.contains("You then said:\nI've read a."), "{msg}");
        assert!(
            msg.contains("still has unchecked steps:\n- [ ] Read a"),
            "{msg}"
        );
        assert!(msg.contains("Tool (echo): hi"), "the work so far: {msg}");
        assert_eq!(
            out.replies.last().unwrap(),
            "- [x] Read a\n- [x] Write b\n\nAll done."
        );
    }

    #[tokio::test]
    async fn assistant2_sends_it_back_twice_at_most() {
        let host = Agentic::with(vec![
            says("- [ ] Do it\n\nHmm."),
            says("- [ ] Do it\n\nHmm again."),
            says("- [ ] Do it\n\nStill hmm."),
            says("never used"),
        ]);
        let out = assist2(&host, "Do it").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(host.calls(), 3);
        assert_eq!(out.replies.last().unwrap(), "- [ ] Do it\n\nStill hmm.");
    }

    #[tokio::test]
    async fn assistant2_stops_to_wait_for_your_answer() {
        let host = Agentic::with(vec![says("- [ ] Edit the file\n\nWhich file do you mean?")]);
        let out = assist2(&host, "Edit the file").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(host.calls(), 1, "a question isn't pushed");
        assert_eq!(
            out.replies,
            ["- [ ] Edit the file\n\nWhich file do you mean?"]
        );
    }

    #[tokio::test]
    async fn assistant2_sends_an_empty_answer_back() {
        let host = Agentic::with(vec![says(""), says("Done.")]);
        let out = assist2(&host, "Do it").await;
        assert_eq!(out.replies, ["Done."]);
        let work = host.work.lock().unwrap();
        assert!(
            work[1].messages[1]
                .text
                .contains("Your last reply was empty")
        );
    }

    #[tokio::test]
    async fn assistant2_summarises_a_long_conversation() {
        let mut host = Agentic::with(
            (1..=8)
                .map(|n| echo("", n))
                .chain([says("Finished.")])
                .collect(),
        );
        host.tool_text = "x".repeat(3900);
        let out = assist2(&host, "Read everything").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(*host.summaries.lock().unwrap(), 1);
        assert!(
            out.replies
                .iter()
                .any(|r| r.contains("summarised it to make room"))
        );
        assert_eq!(out.replies.last().unwrap(), "Finished.");
        // After the summary, the work goes on in a fresh, short exchange.
        let work = host.work.lock().unwrap();
        let i = work
            .iter()
            .position(|c| c.messages[1].text.contains("SUMMARY of the work"))
            .expect("a call carrying on from the summary");
        assert!(i >= 5, "summarised too early: call {i}");
        assert!(work[i].messages[1].text.contains("Read everything"));
        assert_eq!(work[i].messages.len(), 2);
    }

    #[tokio::test]
    async fn assistant2_stops_with_the_reason_when_the_model_fails() {
        let host = Agentic {
            fail_on: Some("you are chummy"),
            ..Agentic::with(vec![])
        };
        let out = assist2(&host, "Do it").await;
        assert!(
            matches!(&out.status, RunStatus::Failed { error, .. } if error.contains("model server failed")),
            "{out:?}"
        );
        assert!(
            out.replies[0].starts_with("**I couldn't finish:**"),
            "{out:?}"
        );
    }
}
