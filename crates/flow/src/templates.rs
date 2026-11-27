//! Starting points offered when creating a loop. Nothing is installed until
//! the user picks one.

use serde::Serialize;

use crate::format::LoopFile;

/// The id of the template new agents get when nothing is configured: the
/// Chummy assistant (medium).
pub const ASSISTANT: &str = "assistant_medium";
/// The Chummy assistant for small models: more steering and checks, a
/// light context.
pub const ASSISTANT_SMALL: &str = "assistant_small";
/// The Chummy assistant for large models: no step decided for it, a big
/// context.
pub const ASSISTANT_LARGE: &str = "assistant_large";

#[derive(Debug, Clone, Serialize)]
pub struct Template {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub file: LoopFile,
}

// The SHA-256 of each file below, worked out by build.rs at every build.
include!(concat!(env!("OUT_DIR"), "/template_prints.rs"));

/// The built-in templates: id, file name in `templates/`, name, description
/// and the file itself. The three assistants are written by
/// `templates/make_templates.py` (from `assistant.py`, `small.py` and
/// `large.py`); the other two are written by hand. Every file must be
/// exactly what Chummy would save (a test checks), so its hash is its
/// fingerprint.
const FILES: &[(&str, &str, &str, &str, &str)] = &[
    (
        ASSISTANT_SMALL,
        "assistant-small.chummy.json",
        "Chummy assistant (small)",
        "For small models (about 1B to 14B). The medium assistant with more help: it decides each step for the model, checks the last change and asks you when it's unclear or going in circles, checks the answer before it's final, and keeps its context light.",
        include_str!("../templates/assistant-small.chummy.json"),
    ),
    (
        ASSISTANT,
        "assistant-medium.chummy.json",
        "Chummy assistant (medium)",
        "The default, for mid-sized models (about 14B to 70B): one agent loop, like a coding agent's. The model works with tools, call after call, until it's done, one decided step at a time. It says what it's doing as it goes, keeps its own to-do list for bigger jobs, reads CHUMMY.md from the folder for your instructions, and summarises long conversations. Sent back to finish if it stops with steps unchecked.",
        include_str!("../templates/assistant-medium.chummy.json"),
    ),
    (
        ASSISTANT_LARGE,
        "assistant-large.chummy.json",
        "Chummy assistant (large)",
        "For large and frontier models: the medium assistant without the steering. The model plans the job and tries to do it in one go, keeps much more in its context to remember the details, and is sent back at most once.",
        include_str!("../templates/assistant-large.chummy.json"),
    ),
    (
        "blank",
        "blank.chummy.json",
        "Blank",
        "A Message block and a Reply block.",
        include_str!("../templates/blank.chummy.json"),
    ),
    (
        "simple_chat",
        "simple-chat.chummy.json",
        "Simple chat",
        "One model call with the conversation as context.",
        include_str!("../templates/simple-chat.chummy.json"),
    ),
];

pub fn templates() -> Vec<Template> {
    FILES
        .iter()
        .map(|(id, _, name, description, json)| Template {
            id,
            name,
            description,
            file: LoopFile::from_json(json)
                .unwrap_or_else(|e| panic!("the built-in template “{id}” is broken: {e}")),
        })
        .collect()
}

/// A loop's fingerprint: the SHA-256 (hex) of the file Chummy would save
/// for it, always in the current format version. Two loops with the same
/// blocks, links, text and layout have the same fingerprint, however their
/// files were spaced or ordered.
pub fn fingerprint(l: &LoopFile) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let json = if l.version == crate::VERSION {
        l.to_json()
    } else {
        let mut c = l.clone();
        c.version = crate::VERSION;
        c.to_json()
    };
    Sha256::digest(json.as_bytes())
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Fingerprints of the built-in templates, worked out at build time.
/// Chummy always trusts these.
pub fn builtin_prints() -> impl Iterator<Item = &'static str> {
    TEMPLATE_PRINTS.iter().map(|(_, p)| *p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::Block;

    #[test]
    fn every_template_is_valid_and_hashed_at_build_time() {
        assert_eq!(
            TEMPLATE_PRINTS.len(),
            FILES.len(),
            "a file in templates/ isn't listed in FILES, or the other way round"
        );
        for (id, file, _, _, json) in FILES {
            let parsed = LoopFile::from_json(json).unwrap();
            assert_eq!(
                parsed.to_json(),
                *json,
                "{file} isn't written exactly as Chummy saves loops; run make_templates.py (or save it from Chummy)"
            );
            let print = TEMPLATE_PRINTS
                .iter()
                .find(|(f, _)| f == file)
                .map(|(_, p)| *p);
            assert_eq!(print, Some(fingerprint(&parsed).as_str()), "{id}");
        }
        let prints: Vec<_> = builtin_prints().collect();
        for t in templates() {
            assert!(prints.contains(&fingerprint(&t.file).as_str()), "{}", t.id);
        }

        // (was templates_are_valid)
        {
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
    use crate::validate::analyze;

    use crate::engine::{
        Host, LlmAnswer, LlmCall, LlmFailure, RunInputs, RunStatus, ToolOutput, run,
    };
    use crate::exchange::{Role, ToolCall, ToolInfo};
    use serde_json::{Value as Json, json};
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

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
    // The Chummy assistant: one agent loop with the model's own to-do list.

    /// A scripted model for the assistant. Work calls (the ones with the
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
        /// The model's context (default: 8,192 tokens, room made at 80%).
        limits: Option<crate::engine::ContextLimits>,
        /// "Say what's next" calls so far.
        nexts: Mutex<u32>,
        /// What Pane output blocks showed, in order.
        pane: Mutex<Vec<String>>,
        /// The next this-many Work calls fail.
        fail_work: Mutex<u32>,
        /// "Report what went wrong" calls, with what they were sent.
        reports: Mutex<Vec<LlmCall>>,
        /// Every model call's messages, in order.
        seen: Mutex<Vec<String>>,
        /// Ask to pause after this many model calls (0: never).
        pause_after: usize,
        /// What "Check the answer" calls say, in order (then "OK").
        check_says: Mutex<Vec<String>>,
        /// "Check the answer" calls so far.
        checks: Mutex<u32>,
        /// What each "Decide what's next" call was asked.
        decided: Mutex<Vec<String>>,
        pause: std::sync::Arc<AtomicBool>,
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
        /// Every message work call `i` sent.
        fn sent(&self, i: usize) -> Vec<String> {
            self.work.lock().unwrap()[i]
                .messages
                .iter()
                .map(|m| m.text.clone())
                .collect()
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
        ) -> Result<LlmAnswer, LlmFailure> {
            let all = call
                .messages
                .iter()
                .map(|m| m.text.to_lowercase())
                .collect::<Vec<_>>()
                .join("\n");
            {
                let mut seen = self.seen.lock().unwrap();
                seen.push(all.clone());
                if seen.len() == self.pause_after {
                    self.pause.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            if let Some(f) = self.fail_on.filter(|f| all.contains(f)) {
                return Err(format!("the model server failed on “{f}”").into());
            }
            let last = call.messages.last().unwrap().text.to_lowercase();
            if last.contains("write a summary to carry on from") {
                assert!(call.tools.is_empty(), "summaries get no tools");
                *self.summaries.lock().unwrap() += 1;
                return Ok(says("SUMMARY of the work"));
            }
            if last.contains("check the answer") {
                assert!(call.tools.is_empty(), "checking gets no tools");
                *self.checks.lock().unwrap() += 1;
                let mut said = self.check_says.lock().unwrap();
                let text = if said.is_empty() {
                    "OK".to_string()
                } else {
                    said.remove(0)
                };
                return Ok(says(&text));
            }
            if last.contains("decide the one next step") {
                assert!(call.tools.is_empty(), "deciding what's next gets no tools");
                self.decided.lock().unwrap().push(last.clone());
                let mut n = self.nexts.lock().unwrap();
                *n += 1;
                // A second line, which only the first line of is kept.
                return Ok(says(&format!(
                    "\n  Next move {n}.\nAnd some rambling after it."
                )));
            }
            if last.contains("what went wrong") {
                assert!(call.tools.is_empty(), "reporting gets no tools");
                self.reports.lock().unwrap().push(call);
                return Ok(says("The server was busy; I'll try the step again."));
            }
            assert!(
                call.messages[0].role == Role::System
                    && call.messages[0].text.starts_with("You are Chummy"),
                "an unexpected call: {call:?}"
            );
            assert!(!call.tools.is_empty(), "the work block gets the tools");
            self.work.lock().unwrap().push(call);
            {
                let mut f = self.fail_work.lock().unwrap();
                if *f > 0 {
                    *f -= 1;
                    return Err("the model server is busy".into());
                }
            }
            Ok(self
                .script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| says("(script ran out)")))
        }
        fn tool_names(&self) -> Vec<String> {
            vec!["echo".into(), "read_file".into(), "run_command".into()]
        }
        fn tool_info(&self, name: &str) -> Option<ToolInfo> {
            let arg = if name == "run_command" {
                "command"
            } else {
                "text"
            };
            ["echo", "read_file", "run_command"]
                .contains(&name)
                .then(|| ToolInfo {
                    name: name.into(),
                    description: "Echoes.".into(),
                    parameters: json!({ "type": "object", "properties": { arg: { "type": "string" } } }),
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
        fn context(&self, _: Option<&str>) -> crate::engine::ContextLimits {
            self.limits.unwrap_or_default()
        }
    }

    async fn assist(host: &Agentic, message: &str) -> crate::engine::RunOutcome {
        assist_with(ASSISTANT, host, message).await
    }

    async fn assist_with(id: &str, host: &Agentic, message: &str) -> crate::engine::RunOutcome {
        let t = templates().into_iter().find(|t| t.id == id).unwrap();
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
        run(&t.file, host, &inputs, &AtomicBool::new(false), |e| {
            if let crate::engine::Event::Pane { text, .. } = e {
                host.pane.lock().unwrap().push(text);
            }
        })
        .await
    }

    #[tokio::test]
    async fn assistant_decides_each_step_and_steers_work_with_it() {
        let host = Agentic::with(vec![echo("I'll look it up.", 1), says("It says hi.")]);
        let out = assist(&host, "What does echo say?").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(host.calls(), 2);
        assert_eq!(*host.nexts.lock().unwrap(), 2, "one before each Work call");
        // One line each in the pane; never in the reply.
        assert_eq!(*host.pane.lock().unwrap(), ["Next move 1.", "Next move 2."]);
        assert_eq!(out.replies, ["I'll look it up.", "It says hi."]);
        // Work's system message names the current step…
        assert!(
            host.system(0)
                .contains("# Your current step\nNext move 1.\n"),
            "{}",
            host.system(0)
        );
        assert!(
            host.system(1)
                .contains("# Your current step\nNext move 2.\n")
        );
        assert!(!host.system(1).contains("rambling"));
        // …and its Reminder sends it last, as a user message.
        for (i, n) in [(0, 1), (1, 2)] {
            let sent = host.sent(i);
            let last = sent.last().unwrap();
            assert!(
                last.contains(&format!("Your step now: Next move {n}.")),
                "{last}"
            );
            assert!(last.contains("not from the user"));
            // Earlier reminders aren't kept in the conversation.
            assert_eq!(
                sent.iter().filter(|t| t.contains("Your step now")).count(),
                1,
                "{sent:#?}"
            );
        }
        // The tool's result is still there, right before the reminder.
        let sent = host.sent(1);
        assert_eq!(sent[sent.len() - 2], host.tool_text, "{sent:#?}");
    }

    #[tokio::test]
    async fn assistant_carries_on_with_the_to_do_list_if_deciding_fails() {
        let host = Agentic {
            fail_on: Some("decide the one next step"),
            ..Agentic::with(vec![echo("Looking.", 1), says("Done.")])
        };
        let out = assist(&host, "Do it").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(host.calls(), 2);
        assert!(host.pane.lock().unwrap().is_empty());
        let fallback = "Carry on with the user's request: do the first unchecked item";
        assert!(host.system(1).contains(fallback), "{}", host.system(1));
        assert!(host.sent(1).last().unwrap().contains(fallback));
    }

    #[tokio::test]
    async fn assistant_answers_straight_away_when_no_tools_are_needed() {
        let host = Agentic::with(vec![says("Paris.")]);
        let out = assist(&host, "What's the capital of France?").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(out.replies, ["Paris."]);
        // Say what's next, then Work.
        assert_eq!(out.llm_calls, 2);
        let sys = host.system(0);
        assert!(sys.contains("# Your to-do list now\n(none yet)"), "{sys}");
        assert!(sys.contains("docs/\nnotes.txt"), "{sys}");
        assert!(
            sys.contains("(none: this folder has no CHUMMY.md)"),
            "{sys}"
        );
        assert!(sys.contains("User: Hello\n\nAssistant: Hi there."), "{sys}");
        // Only the request goes in the exchange; the rest is the system
        // message, and the step to take comes last (not kept).
        let first = &host.work.lock().unwrap()[0].messages;
        assert_eq!(first.len(), 3);
        assert_eq!(first[1].text, "What's the capital of France?");
        assert!(first[2].text.contains("Your step now"));
    }

    #[tokio::test]
    async fn assistant_reads_chummy_md() {
        let mut host = Agentic::with(vec![says("Bonjour.")]);
        host.files
            .insert("CHUMMY.md".into(), "Always answer in French.".into());
        let out = assist(&host, "Hi").await;
        assert_eq!(out.replies, ["Bonjour."]);
        assert!(
            host.system(0)
                .contains("(CHUMMY.md)\nAlways answer in French.")
        );
    }

    #[tokio::test]
    async fn assistant_loops_through_tools_and_says_what_its_doing() {
        let host = Agentic::with(vec![
            echo("I'll look it up.", 1),
            echo("", 2),
            says("It says hi, twice."),
        ]);
        let out = assist(&host, "What does echo say?").await;
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

    /// Runs the assistant able to pause (by `host.pause`), or carries on
    /// `resume`.
    async fn assist_pausable(
        host: &Agentic,
        message: &str,
        resume: Option<crate::engine::RunState>,
    ) -> crate::engine::RunOutcome {
        let t = templates().into_iter().find(|t| t.id == ASSISTANT).unwrap();
        let inputs = RunInputs {
            message: message.into(),
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        crate::engine::run_with(
            &t.file,
            host,
            &inputs,
            resume,
            &cancel,
            Some(&host.pause),
            None,
            |_| {},
        )
        .await
    }

    #[tokio::test]
    async fn the_assistant_carries_on_the_same_after_a_pause_anywhere() {
        let script = || {
            vec![
                echo("I'll look it up.", 1),
                echo("", 2),
                says("It says hi, twice."),
            ]
        };
        let ask = "What does echo say?";
        let whole = Agentic::with(script());
        let all = assist_pausable(&whole, ask, None).await;
        assert_eq!(all.status, RunStatus::Completed, "{all:?}");
        let calls = whole.seen.lock().unwrap().len();
        assert!(calls >= 5, "{calls}");
        // Pause after every model call but the last, save and read back the
        // state, and carry on: the same calls, tools and replies as without.
        for k in 1..calls {
            let host = Agentic {
                pause_after: k,
                ..Agentic::with(script())
            };
            let first = assist_pausable(&host, ask, None).await;
            assert!(
                matches!(first.status, RunStatus::Paused { reason: None }),
                "after call {k}: {first:?}"
            );
            let state = first.paused.as_deref().unwrap();
            let state = serde_json::from_str(&serde_json::to_string(state).unwrap()).unwrap();
            host.pause
                .store(false, std::sync::atomic::Ordering::Relaxed);
            let rest = assist_pausable(&host, ask, Some(state)).await;
            assert_eq!(
                rest.status,
                RunStatus::Completed,
                "after call {k}: {rest:?}"
            );
            assert_eq!(rest.replies, all.replies, "after call {k}");
            assert_eq!(
                *host.seen.lock().unwrap(),
                *whole.seen.lock().unwrap(),
                "after call {k}"
            );
            assert_eq!(
                *host.tools_run.lock().unwrap(),
                *whole.tools_run.lock().unwrap()
            );
            assert_eq!(
                (rest.steps, rest.llm_calls, rest.tool_calls),
                (all.steps, all.llm_calls, all.tool_calls)
            );
        }
    }

    #[tokio::test]
    async fn assistant_keeps_the_models_own_todo_list() {
        let host = Agentic::with(vec![
            echo("I'll do it in two steps.\n\n- [ ] Read a\n- [ ] Write b", 1),
            echo("- [x] Read a\n- [ ] Write b", 2),
            says("- [x] Read a\n- [x] Write b\n\nAll done."),
        ]);
        let out = assist(&host, "Copy a to b").await;
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
    async fn assistant_sends_it_back_when_steps_are_unchecked() {
        let host = Agentic::with(vec![
            echo("- [ ] Read a\n- [ ] Write b", 1),
            says("I've read a."),
            echo("", 2),
            says("- [x] Read a\n- [x] Write b\n\nAll done."),
        ]);
        let out = assist(&host, "Copy a to b").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(host.calls(), 4);
        // The early stop is shown, then the model is told to carry on, in a
        // fresh exchange holding the request, the work so far and the list.
        assert_eq!(out.replies[1], "I've read a.");
        let work = host.work.lock().unwrap().clone();
        let back = &work[2].messages;
        // The fresh exchange, then the step reminder.
        assert_eq!(back.len(), 3, "{back:?}");
        let msg = &back[1].text;
        assert!(msg.contains("The user's request:\nCopy a to b"), "{msg}");
        assert!(msg.contains("You then said:\nI've read a."), "{msg}");
        assert!(
            msg.contains("still has unchecked steps:\n- [ ] Read a"),
            "{msg}"
        );
        assert!(msg.contains("Tool (echo): hi"), "the work so far: {msg}");
        assert!(
            !msg.contains("You are Chummy"),
            "progress leaves out the instructions"
        );
        assert_eq!(
            out.replies.last().unwrap(),
            "- [x] Read a\n- [x] Write b\n\nAll done."
        );

        // (was assistant_sends_it_back_twice_at_most)
        {
            let host = Agentic::with(vec![
                says("- [ ] Do it\n\nHmm."),
                says("- [ ] Do it\n\nHmm again."),
                says("- [ ] Do it\n\nStill hmm."),
                says("never used"),
            ]);
            let out = assist(&host, "Do it").await;
            assert_eq!(out.status, RunStatus::Completed, "{out:?}");
            assert_eq!(host.calls(), 3);
            assert_eq!(out.replies.last().unwrap(), "- [ ] Do it\n\nStill hmm.");
        }
    }

    #[tokio::test]
    async fn assistant_stops_to_wait_for_your_answer() {
        let host = Agentic::with(vec![says("- [ ] Edit the file\n\nWhich file do you mean?")]);
        let out = assist(&host, "Edit the file").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(host.calls(), 1, "a question isn't pushed");
        assert_eq!(
            out.replies,
            ["- [ ] Edit the file\n\nWhich file do you mean?"]
        );
    }

    #[tokio::test]
    async fn assistant_sends_an_empty_answer_back() {
        let host = Agentic::with(vec![says(""), says("Done.")]);
        let out = assist(&host, "Do it").await;
        assert_eq!(out.replies, ["Done."]);
        let work = host.work.lock().unwrap();
        assert!(
            work[1].messages[1]
                .text
                .contains("Your last reply was empty")
        );
    }

    #[tokio::test]
    async fn assistant_summarises_a_long_conversation() {
        let mut host = Agentic::with(
            (1..=8)
                .map(|n| echo("", n))
                .chain([says("Finished.")])
                .collect(),
        );
        host.tool_text = "x".repeat(3900);
        let out = assist(&host, "Read everything").await;
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
        // ~980 tokens a result: the default point (6,553 of 8,192) comes after
        // about six of them.
        assert!((5..=7).contains(&i), "summarised at call {i}");
        assert!(work[i].messages[1].text.contains("Read everything"));
        assert_eq!(work[i].messages.len(), 3, "system, summary, step reminder");
        assert!(work[i].messages[0].text.starts_with("You are Chummy"));
    }

    #[tokio::test]
    async fn assistant_makes_room_at_the_models_own_point() {
        let mut host = Agentic::with(
            (1..=8)
                .map(|n| echo("", n))
                .chain([says("Finished.")])
                .collect(),
        );
        host.tool_text = "x".repeat(3900);
        host.limits = Some(crate::engine::ContextLimits {
            window: 4096,
            compact_at: 2500,
        });
        let out = assist(&host, "Read everything").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        let work = host.work.lock().unwrap();
        let i = work
            .iter()
            .position(|c| c.messages[1].text.contains("SUMMARY of the work"))
            .expect("a summary");
        assert!(i <= 3, "summarised at call {i}");
        // The turn's figures: context measured against this model.
        assert_eq!(out.usage.context_window, 4096);
        assert!(out.usage.tokens_added > 8 * 900, "{:?}", out.usage);
    }

    #[tokio::test]
    async fn assistant_stops_with_the_reason_when_the_model_fails() {
        let host = Agentic {
            fail_on: Some("you are chummy"),
            ..Agentic::with(vec![])
        };
        let out = assist(&host, "Do it").await;
        assert!(
            matches!(&out.status, RunStatus::Failed { error, .. } if error.contains("model server failed")),
            "{out:?}"
        );
        // Two reports (the report fails too here, so the error itself is
        // shown), then it gives up on the third failure.
        let tries: Vec<&String> = out
            .replies
            .iter()
            .filter(|r| r.starts_with("_Something went wrong:"))
            .collect();
        assert_eq!(tries.len(), 2, "{out:?}");
        assert!(tries[0].contains("model server failed"));
        let last = out.replies.last().unwrap();
        assert!(
            last.starts_with("**I couldn't finish:** It failed 3 times"),
            "{last}"
        );
    }

    #[tokio::test]
    async fn assistant_reports_a_failure_and_tries_again() {
        let host = Agentic::with(vec![says("All done.")]);
        *host.fail_work.lock().unwrap() = 1;
        let out = assist(&host, "Do it").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        // The report is shown, then the final answer.
        assert_eq!(
            out.replies,
            [
                "_Something went wrong: The server was busy; I'll try the step again. Trying again._",
                "All done."
            ]
        );
        assert_eq!(*host.pane.lock().unwrap().last().unwrap(), "Next move 2.");
        assert!(
            host.pane
                .lock()
                .unwrap()
                .contains(&"The server was busy; I'll try the step again.".to_string())
        );
        // The reporter saw the failure as the last message…
        let reports = host.reports.lock().unwrap();
        assert_eq!(reports.len(), 1);
        let sent: Vec<&str> = reports[0]
            .messages
            .iter()
            .map(|m| m.text.as_str())
            .collect();
        assert_eq!(
            sent[sent.len() - 2],
            "(The last step failed: the model server is busy)"
        );
        // …but the retry doesn't: same messages as the first try, and the
        // next step knows what happened.
        let work = host.work.lock().unwrap();
        assert_eq!(work.len(), 2);
        let texts = |i: usize| -> Vec<String> {
            work[i]
                .messages
                .iter()
                .skip(1)
                .map(|m| m.text.clone())
                .collect()
        };
        assert_eq!(texts(0).len(), texts(1).len(), "{:#?}", texts(1));
        assert!(texts(1).iter().all(|t| !t.contains("last step failed")));
        assert_eq!(*host.nexts.lock().unwrap(), 2);
    }

    // ------------------------------------------------------------------
    // The small and large assistants: the same loop, with more help or less.

    #[tokio::test]
    async fn small_assistant_checks_the_answer_and_sends_it_back_to_fix_it() {
        let host = Agentic::with(vec![
            echo("I'll look.", 1),
            says("It says bye."),
            says("It says hi."),
        ]);
        *host.check_says.lock().unwrap() =
            vec!["Problem: the tool said hi, not bye.".into(), "OK.".into()];
        let out = assist_with(ASSISTANT_SMALL, &host, "What does echo say?").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(out.replies.last().unwrap(), "It says hi.");
        assert_eq!(
            *host.checks.lock().unwrap(),
            2,
            "each final answer is checked"
        );
        // The check went back to the model, and is in the pane.
        let sent = host.sent(2).join("\n");
        assert!(sent.contains("the tool said hi, not bye"), "{sent}");
        assert!(
            host.pane
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.starts_with("Problem:"))
        );
        // A light context, and steering that knows to check and to ask.
        assert!(host.system(0).contains("ask the user one short question"));
        let next = host.decided.lock().unwrap()[0].clone();
        assert!(next.contains("ask the user"), "{next}");
    }

    #[tokio::test]
    async fn small_assistant_takes_the_answer_if_the_check_fails() {
        let host = Agentic {
            fail_on: Some("check the answer"),
            ..Agentic::with(vec![says("Paris.")])
        };
        let out = assist_with(ASSISTANT_SMALL, &host, "Capital of France?").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(out.replies, ["Paris."]);
    }

    #[tokio::test]
    async fn large_assistant_works_without_being_steered() {
        let host = Agentic::with(vec![
            echo("I'll look it up.", 1),
            echo("", 2),
            says("It says hi, twice."),
        ]);
        let out = assist_with(ASSISTANT_LARGE, &host, "What does echo say?").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(out.replies, ["I'll look it up.", "It says hi, twice."]);
        // No step decided, no reminder, no check: only Work calls.
        assert_eq!(*host.nexts.lock().unwrap(), 0);
        assert_eq!(*host.checks.lock().unwrap(), 0);
        assert_eq!(out.llm_calls, 3);
        assert!(host.pane.lock().unwrap().is_empty());
        for i in 0..3 {
            assert!(host.sent(i).iter().all(|t| !t.contains("Your step now")));
        }
        let sys = host.system(0);
        assert!(!sys.contains("# Your current step"), "{sys}");
        assert!(
            sys.contains("# If your last try failed\n(nothing has failed)"),
            "{sys}"
        );
    }

    #[tokio::test]
    async fn large_assistant_is_sent_back_once_at_most() {
        let host = Agentic::with(vec![
            says("- [x] one\n- [ ] two\nDone with one."),
            says("- [x] one\n- [ ] two\nStill not two."),
        ]);
        let out = assist_with(ASSISTANT_LARGE, &host, "Do one and two").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(host.calls(), 2, "sent back once, then taken");
        assert_eq!(
            out.replies.last().unwrap(),
            "- [x] one\n- [ ] two\nStill not two."
        );
    }

    #[tokio::test]
    async fn large_assistant_tells_the_next_try_what_failed() {
        let host = Agentic::with(vec![says("All done.")]);
        *host.fail_work.lock().unwrap() = 1;
        let out = assist_with(ASSISTANT_LARGE, &host, "Do it").await;
        assert_eq!(out.status, RunStatus::Completed, "{out:?}");
        assert_eq!(out.replies.last().unwrap(), "All done.");
        assert_eq!(host.reports.lock().unwrap().len(), 1);
        let sys = host.system(1);
        assert!(
            sys.contains("# If your last try failed\nThe last try failed: The server was busy"),
            "{sys}"
        );
    }
}
