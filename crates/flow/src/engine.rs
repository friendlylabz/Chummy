//! The loop engine.
//!
//! Execution model:
//!
//! - Every input port has a queue. A value arriving on any link into the port
//!   joins the back of its queue.
//! - A block is ready when **every** input port has at least one value. It then
//!   takes one value from the front of each queue and runs.
//! - A block's results go to **every** link from the output port that produced
//!   them (broadcast). Most blocks produce one value on each output; *For each*
//!   produces one per item, *Branch* uses only one of its two outputs, and
//!   *Reply* produces none.
//! - Blocks without inputs (Message, Conversation, Fixed text) run once, at the
//!   start.
//! - Ready blocks run one at a time, first come, first served, so a run is
//!   deterministic. The run ends when nothing is ready.
//! - Cycles are allowed: a block runs again whenever its ports refill. Runs are
//!   bounded by [`RunLimits`] and can be stopped at any time.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::blocks::BlockKind;
use crate::format::LoopFile;
use crate::text::{fill_prompt, split, strip_thinking};
use crate::validate::{Severity, analyze};

/// A value flowing between blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Value {
    Text(String),
    List(Vec<String>),
}

impl Value {
    pub fn as_text(&self) -> String {
        match self {
            Value::Text(t) => t.clone(),
            Value::List(l) => l.join("\n"),
        }
    }

    pub fn as_list(&self) -> Vec<String> {
        match self {
            Value::Text(t) => vec![t.clone()],
            Value::List(l) => l.clone(),
        }
    }

    /// Short preview for the UI.
    pub fn preview(&self) -> String {
        const MAX: usize = 240;
        let s = match self {
            Value::Text(t) => t.clone(),
            Value::List(l) => format!(
                "[{} items] {}",
                l.len(),
                l.first().cloned().unwrap_or_default()
            ),
        };
        if s.chars().count() > MAX {
            s.chars().take(MAX).collect::<String>() + "…"
        } else {
            s
        }
    }
}

fn d_steps() -> u32 {
    200
}
fn d_llm() -> u32 {
    20
}

/// Safety limits for one run. Stored in the loop file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunLimits {
    /// Most block runs allowed in one run of the loop.
    #[serde(default = "d_steps")]
    pub max_steps: u32,
    /// Most model calls allowed in one run of the loop.
    #[serde(default = "d_llm")]
    pub max_llm_calls: u32,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_steps: d_steps(),
            max_llm_calls: d_llm(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTurn {
    /// "user" or "assistant".
    pub role: String,
    pub text: String,
}

/// What a run starts with.
#[derive(Debug, Clone, Default)]
pub struct RunInputs {
    /// The chat message; every Message block sends it.
    pub message: String,
    /// Earlier conversation, oldest first.
    pub history: Vec<ChatTurn>,
    /// Per-block text for specific Message blocks (editor test runs).
    pub overrides: HashMap<String, String>,
}

/// One model request from an LLM block.
#[derive(Debug, Clone, PartialEq)]
pub struct LlmCall {
    pub block: String,
    pub prompt: String,
    pub system: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: u32,
    /// Model connection id; `None` means the agent's model.
    pub model: Option<String>,
}

/// What the engine needs from the outside world.
pub trait Host: Send + Sync {
    /// Asks a model. `on_text` receives the answer as it streams.
    fn llm(
        &self,
        call: LlmCall,
        on_text: &mut (dyn FnMut(&str) + Send),
    ) -> impl Future<Output = Result<String, String>> + Send;
    fn read_file(&self, path: &str) -> Result<String, String>;
    fn write_file(&self, path: &str, content: &str, append: bool) -> Result<(), String>;
    /// Names in a folder; folders end with `/`.
    fn list_files(&self, path: &str) -> Result<Vec<String>, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockState {
    /// Some inputs have arrived; waiting for the rest.
    Waiting,
    Running,
    Done,
    Failed,
}

/// Progress reported while a run goes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    BlockState {
        block: String,
        label: String,
        state: BlockState,
    },
    BlockOutput {
        block: String,
        port: String,
        preview: String,
    },
    /// Streaming text from a model.
    LlmText { block: String, text: String },
    /// A Reply block produced part of the answer.
    Reply { block: String, text: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    Stopped,
    LimitReached {
        message: String,
    },
    Failed {
        block: Option<String>,
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunOutcome {
    pub status: RunStatus,
    /// Everything Reply blocks produced, in order.
    pub replies: Vec<String>,
    pub steps: u32,
    pub llm_calls: u32,
    /// Blocks left with some, but not all, of their inputs.
    pub waiting: Vec<String>,
}

type Outputs = Vec<(String, Value)>;

struct Runner<'a, H: Host> {
    l: &'a LoopFile,
    host: &'a H,
    inputs: &'a RunInputs,
    cancel: &'a AtomicBool,
    kinds: HashMap<String, BlockKind>,
    /// Input port names per block, in order.
    in_ports: HashMap<String, Vec<String>>,
    /// Per block, one queue per input port.
    queues: HashMap<String, Vec<VecDeque<Value>>>,
    /// (block, output port) → [(block, input index)]
    routes: HashMap<(String, String), Vec<(String, usize)>>,
    ready: VecDeque<String>,
    /// How many times each block is in `ready`. Kept equal to the number of
    /// runs its queued inputs allow, so the queue is strictly first-come.
    scheduled: HashMap<String, usize>,
    remembered: HashMap<String, Value>,
    replies: Vec<String>,
    steps: u32,
    llm_calls: u32,
}

/// Runs `l` to completion (or until stopped or limited).
pub async fn run<H: Host>(
    l: &LoopFile,
    host: &H,
    inputs: &RunInputs,
    cancel: &AtomicBool,
    mut on_event: impl FnMut(Event) + Send,
) -> RunOutcome {
    let analysis = analyze(l);
    if let Some(e) = analysis
        .issues
        .iter()
        .find(|i| i.severity == Severity::Error)
    {
        return RunOutcome {
            status: RunStatus::Failed {
                block: e.block.clone(),
                error: e.message.clone(),
            },
            replies: vec![],
            steps: 0,
            llm_calls: 0,
            waiting: vec![],
        };
    }

    let mut r = Runner {
        l,
        host,
        inputs,
        cancel,
        kinds: HashMap::new(),
        in_ports: HashMap::new(),
        queues: HashMap::new(),
        routes: HashMap::new(),
        ready: VecDeque::new(),
        scheduled: HashMap::new(),
        remembered: HashMap::new(),
        replies: vec![],
        steps: 0,
        llm_calls: 0,
    };
    for b in &l.blocks {
        // Validation passed, so every block parses.
        let Ok(k) = BlockKind::parse(&b.kind, &b.params) else {
            continue;
        };
        let ins: Vec<String> = k.inputs().into_iter().map(|p| p.name).collect();
        r.queues
            .insert(b.id.clone(), vec![VecDeque::new(); ins.len()]);
        if ins.is_empty() {
            r.ready.push_back(b.id.clone());
        }
        r.in_ports.insert(b.id.clone(), ins);
        r.kinds.insert(b.id.clone(), k);
    }
    for link in &l.links {
        let idx = r.in_ports[&link.to.block]
            .iter()
            .position(|p| *p == link.to.port)
            .unwrap_or(0);
        r.routes
            .entry((link.from.block.clone(), link.from.port.clone()))
            .or_default()
            .push((link.to.block.clone(), idx));
    }

    let status = r.drive(&mut on_event).await;
    let waiting = r
        .queues
        .iter()
        .filter(|(_, qs)| qs.iter().any(|q| !q.is_empty()) && qs.iter().any(|q| q.is_empty()))
        .map(|(id, _)| id.clone())
        .collect();
    RunOutcome {
        status,
        replies: r.replies,
        steps: r.steps,
        llm_calls: r.llm_calls,
        waiting,
    }
}

async fn cancelled(flag: &AtomicBool) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

impl<H: Host> Runner<'_, H> {
    /// How many more runs the queued inputs allow.
    fn possible_runs(&self, id: &str) -> usize {
        self.queues[id].iter().map(|q| q.len()).min().unwrap_or(0)
    }

    /// Queues `id` once for every run its inputs now allow.
    fn schedule(&mut self, id: &str) -> bool {
        let possible = self.possible_runs(id);
        let s = self.scheduled.entry(id.to_string()).or_default();
        let added = *s < possible;
        while *s < possible {
            *s += 1;
            self.ready.push_back(id.to_string());
        }
        added
    }

    fn label(&self, id: &str) -> String {
        self.l
            .block(id)
            .map(|b| {
                if b.label.is_empty() {
                    id.to_string()
                } else {
                    b.label.clone()
                }
            })
            .unwrap_or_else(|| id.to_string())
    }

    async fn drive(&mut self, on_event: &mut (impl FnMut(Event) + Send)) -> RunStatus {
        while let Some(id) = self.ready.pop_front() {
            if self.cancel.load(Ordering::Relaxed) {
                return RunStatus::Stopped;
            }
            if let Some(s) = self.scheduled.get_mut(&id) {
                *s = s.saturating_sub(1);
            }
            let args: Vec<Value> = self
                .queues
                .get_mut(&id)
                .unwrap()
                .iter_mut()
                .filter_map(|q| q.pop_front())
                .collect();

            self.steps += 1;
            if self.steps > self.l.limits.max_steps {
                return RunStatus::LimitReached {
                    message: format!(
                        "Stopped after {} steps, the loop's limit.",
                        self.l.limits.max_steps
                    ),
                };
            }

            on_event(Event::BlockState {
                block: id.clone(),
                label: self.label(&id),
                state: BlockState::Running,
            });
            let result = self.execute(&id, args, on_event).await;
            let outputs = match result {
                Ok(o) => o,
                Err(Stop::Cancelled) => return RunStatus::Stopped,
                Err(Stop::Limit(message)) => return RunStatus::LimitReached { message },
                Err(Stop::Failed(e)) => {
                    on_event(Event::BlockState {
                        block: id.clone(),
                        label: self.label(&id),
                        state: BlockState::Failed,
                    });
                    return RunStatus::Failed {
                        block: Some(id.clone()),
                        error: format!("{}: {e}", self.label(&id)),
                    };
                }
            };
            on_event(Event::BlockState {
                block: id.clone(),
                label: self.label(&id),
                state: BlockState::Done,
            });

            for (port, value) in outputs {
                on_event(Event::BlockOutput {
                    block: id.clone(),
                    port: port.clone(),
                    preview: value.preview(),
                });
                let targets = self
                    .routes
                    .get(&(id.clone(), port))
                    .cloned()
                    .unwrap_or_default();
                for (to, idx) in targets {
                    self.queues.get_mut(&to).unwrap()[idx].push_back(value.clone());
                    if !self.schedule(&to) && self.possible_runs(&to) == 0 {
                        on_event(Event::BlockState {
                            label: self.label(&to),
                            block: to,
                            state: BlockState::Waiting,
                        });
                    }
                }
            }
        }
        RunStatus::Completed
    }

    async fn execute(
        &mut self,
        id: &str,
        args: Vec<Value>,
        on_event: &mut (impl FnMut(Event) + Send),
    ) -> Result<Outputs, Stop> {
        let one = |v: Value| Ok(vec![("out".to_string(), v)]);
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Text(String::new()));
        let kind = self.kinds[id].clone();
        match kind {
            BlockKind::Input(_) => {
                let text = self
                    .inputs
                    .overrides
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| self.inputs.message.clone());
                one(Value::Text(text))
            }
            BlockKind::History(p) => {
                let n = p.max_messages as usize;
                let h = &self.inputs.history;
                let recent = &h[h.len().saturating_sub(n)..];
                let text = if recent.is_empty() {
                    "(This is the start of the conversation.)".to_string()
                } else {
                    recent
                        .iter()
                        .map(|t| {
                            format!(
                                "{}: {}",
                                if t.role == "user" {
                                    "User"
                                } else {
                                    "Assistant"
                                },
                                t.text
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n")
                };
                one(Value::Text(text))
            }
            BlockKind::Fixed(p) => one(Value::Text(p.text)),
            BlockKind::Output(_) => {
                let text = arg(0).as_text();
                self.replies.push(text.clone());
                on_event(Event::Reply {
                    block: id.to_string(),
                    text,
                });
                Ok(vec![])
            }
            BlockKind::Llm(p) => {
                if self.llm_calls >= self.l.limits.max_llm_calls {
                    return Err(Stop::Limit(format!(
                        "Stopped after {} model calls, the loop's limit.",
                        self.l.limits.max_llm_calls
                    )));
                }
                self.llm_calls += 1;
                let call = LlmCall {
                    block: id.to_string(),
                    prompt: fill_prompt(&p.prompt, &arg(0).as_text()),
                    system: Some(p.system.clone()).filter(|s| !s.trim().is_empty()),
                    temperature: p.temperature,
                    max_tokens: p.max_tokens,
                    model: p.model.clone().filter(|m| !m.is_empty()),
                };
                let block = id.to_string();
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                let mut on_text = move |t: &str| {
                    let _ = tx.send(t.to_string());
                };
                let fut = self.host.llm(call, &mut on_text);
                tokio::pin!(fut);
                let answer = loop {
                    tokio::select! {
                        r = &mut fut => break r,
                        Some(t) = rx.recv() => on_event(Event::LlmText { block: block.clone(), text: t }),
                        _ = cancelled(self.cancel) => return Err(Stop::Cancelled),
                    }
                };
                while let Ok(t) = rx.try_recv() {
                    on_event(Event::LlmText {
                        block: block.clone(),
                        text: t,
                    });
                }
                one(Value::Text(strip_thinking(&answer.map_err(Stop::Failed)?)))
            }
            BlockKind::Combine(p) => {
                let mut out = p.format.clone();
                for i in (1..=args.len()).rev() {
                    out = out.replace(&format!("_INPUT{i}_"), &args[i - 1].as_text());
                }
                one(Value::Text(out))
            }
            BlockKind::Forward(_) => one(arg(0)),
            BlockKind::Remember(_) => {
                let v = self
                    .remembered
                    .entry(id.to_string())
                    .or_insert_with(|| arg(0))
                    .clone();
                one(v)
            }
            BlockKind::Split(p) => one(Value::List(split(
                &arg(0).as_text(),
                p.chunk_size as usize,
                p.overlap as usize,
                p.deviation as usize,
            ))),
            BlockKind::ForEach(_) => Ok(arg(0)
                .as_list()
                .into_iter()
                .map(|t| ("out".to_string(), Value::Text(t)))
                .collect()),
            BlockKind::Separate(p) => Ok(arg(0)
                .as_list()
                .into_iter()
                .take(p.outputs as usize)
                .enumerate()
                .map(|(i, t)| (format!("out{}", i + 1), Value::Text(t)))
                .collect()),
            BlockKind::Join(p) => {
                let sep = p.separator.replace("\\n", "\n").replace("\\t", "\t");
                one(Value::Text(arg(0).as_list().join(&sep)))
            }
            BlockKind::Regex(p) => {
                let re = regex::Regex::new(&p.pattern).map_err(|e| Stop::Failed(e.to_string()))?;
                let text = arg(0).as_text();
                one(Value::List(
                    re.find_iter(&text)
                        .map(|m| m.as_str().to_string())
                        .collect(),
                ))
            }
            BlockKind::Capture(p) => {
                let re = regex::Regex::new(&p.pattern).map_err(|e| Stop::Failed(e.to_string()))?;
                let text = arg(0).as_text();
                let groups = (re.captures_len() - 1).min(10);
                let mut cols = vec![Vec::new(); groups];
                for caps in re.captures_iter(&text) {
                    for (g, col) in cols.iter_mut().enumerate() {
                        col.push(
                            caps.get(g + 1)
                                .map(|m| m.as_str().to_string())
                                .unwrap_or_default(),
                        );
                    }
                }
                Ok(cols
                    .into_iter()
                    .enumerate()
                    .map(|(i, c)| (format!("group{}", i + 1), Value::List(c)))
                    .collect())
            }
            BlockKind::Branch(p) => {
                let re = regex::Regex::new(&p.pattern).map_err(|e| Stop::Failed(e.to_string()))?;
                let port = if re.is_match(&arg(1).as_text()) {
                    "yes"
                } else {
                    "no"
                };
                Ok(vec![(port.to_string(), arg(0))])
            }
            BlockKind::ReadFile(_) => one(Value::Text(
                self.host
                    .read_file(arg(0).as_text().trim())
                    .map_err(Stop::Failed)?,
            )),
            BlockKind::WriteFile(p) => {
                let path = arg(0).as_text().trim().to_string();
                self.host
                    .write_file(&path, &arg(1).as_text(), p.append)
                    .map_err(Stop::Failed)?;
                one(Value::Text(path))
            }
            BlockKind::ListFiles(_) => one(Value::List(
                self.host
                    .list_files(arg(0).as_text().trim())
                    .map_err(Stop::Failed)?,
            )),
        }
    }
}

enum Stop {
    Cancelled,
    Limit(String),
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{Block, Link, PortRef, Position};
    use serde_json::json;
    use std::sync::Mutex;

    /// A model that answers from a script and records prompts.
    #[derive(Default)]
    struct Fake {
        answers: Mutex<VecDeque<String>>,
        prompts: Mutex<Vec<String>>,
        files: Mutex<HashMap<String, String>>,
    }

    impl Fake {
        fn with(answers: &[&str]) -> Self {
            let f = Self::default();
            f.answers
                .lock()
                .unwrap()
                .extend(answers.iter().map(|s| s.to_string()));
            f
        }
    }

    impl Host for Fake {
        async fn llm(
            &self,
            call: LlmCall,
            on_text: &mut (dyn FnMut(&str) + Send),
        ) -> Result<String, String> {
            self.prompts.lock().unwrap().push(call.prompt.clone());
            let a = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| format!("echo: {}", call.prompt));
            on_text(&a);
            Ok(a)
        }
        fn read_file(&self, path: &str) -> Result<String, String> {
            self.files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| format!("{path}: not found"))
        }
        fn write_file(&self, path: &str, content: &str, append: bool) -> Result<(), String> {
            let mut f = self.files.lock().unwrap();
            let e = f.entry(path.to_string()).or_default();
            if !append {
                e.clear();
            }
            e.push_str(content);
            Ok(())
        }
        fn list_files(&self, _path: &str) -> Result<Vec<String>, String> {
            Ok(self.files.lock().unwrap().keys().cloned().collect())
        }
    }

    fn b(id: &str, kind: &str, params: serde_json::Value) -> Block {
        Block {
            id: id.into(),
            kind: kind.into(),
            label: id.into(),
            position: Position::default(),
            params,
        }
    }
    fn ln(a: &str, ap: &str, c: &str, cp: &str) -> Link {
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
    fn lp(blocks: Vec<Block>, links: Vec<Link>) -> LoopFile {
        let mut l = LoopFile::new("t");
        l.blocks = blocks;
        l.links = links;
        l
    }
    async fn go(l: &LoopFile, host: &Fake, msg: &str) -> (RunOutcome, Vec<Event>) {
        let mut ev = vec![];
        let inputs = RunInputs {
            message: msg.into(),
            ..Default::default()
        };
        let out = run(l, host, &inputs, &AtomicBool::new(false), |e| ev.push(e)).await;
        (out, ev)
    }

    #[tokio::test]
    async fn linear_loop_replies() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("llm", "llm", json!({ "prompt": "Q: _INPUT_" })),
                b("out", "output", json!({})),
            ],
            vec![ln("in", "out", "llm", "in"), ln("llm", "out", "out", "in")],
        );
        let host = Fake::with(&["<think>hmm</think>A!"]);
        let (o, ev) = go(&l, &host, "hello").await;
        assert_eq!(o.status, RunStatus::Completed);
        assert_eq!(o.replies, vec!["A!"]);
        assert_eq!(host.prompts.lock().unwrap()[0], "Q: hello");
        assert!(ev.iter().any(|e| matches!(e, Event::LlmText { .. })));
        assert_eq!((o.steps, o.llm_calls), (3, 1));
    }

    #[tokio::test]
    async fn combine_waits_for_all_inputs() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("fx", "fixed", json!({ "text": "world" })),
                b(
                    "c",
                    "combine",
                    json!({ "inputs": 2, "format": "_INPUT1_, _INPUT2_" }),
                ),
                b("out", "output", json!({})),
            ],
            vec![
                ln("in", "out", "c", "in1"),
                ln("fx", "out", "c", "in2"),
                ln("c", "out", "out", "in"),
            ],
        );
        let (o, ev) = go(&l, &Fake::default(), "hello").await;
        assert_eq!(o.replies, vec!["hello, world"]);
        assert!(ev.contains(&Event::BlockState {
            block: "c".into(),
            label: "c".into(),
            state: BlockState::Waiting
        }));
    }

    #[tokio::test]
    async fn broadcasts_and_queues_multiple_arrivals() {
        // Two links into one port: the block runs once per arrival.
        let l = lp(
            vec![
                b("a", "fixed", json!({ "text": "one" })),
                b("b", "fixed", json!({ "text": "two" })),
                b("f", "forward", json!({})),
                b("o1", "output", json!({})),
            ],
            vec![
                ln("a", "out", "f", "in"),
                ln("b", "out", "f", "in"),
                ln("f", "out", "o1", "in"),
            ],
        );
        let (o, _) = go(&l, &Fake::default(), "").await;
        assert_eq!(o.replies, vec!["one", "two"]);
    }

    #[tokio::test]
    async fn self_check_cycle_retries_until_yes() {
        // draft → check → branch(yes → reply, no → improve → back to check)
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("draft", "llm", json!({ "prompt": "draft _INPUT_" })),
                b("check", "llm", json!({ "prompt": "check _INPUT_" })),
                b("br", "branch", json!({ "pattern": "(?i)^yes" })),
                b("improve", "llm", json!({ "prompt": "improve _INPUT_" })),
                b("out", "output", json!({})),
            ],
            vec![
                ln("in", "out", "draft", "in"),
                ln("draft", "out", "br", "value"),
                ln("draft", "out", "check", "in"),
                ln("check", "out", "br", "test"),
                ln("br", "yes", "out", "in"),
                ln("br", "no", "improve", "in"),
                ln("improve", "out", "br", "value"),
                ln("improve", "out", "check", "in"),
            ],
        );
        let host = Fake::with(&["v1", "NO", "v2", "Yes, good"]);
        let (o, _) = go(&l, &host, "task").await;
        assert_eq!(o.status, RunStatus::Completed);
        assert_eq!(o.replies, vec!["v2"]);
        assert_eq!(o.llm_calls, 4);
    }

    #[tokio::test]
    async fn infinite_cycles_hit_the_limit() {
        let mut l = lp(
            vec![
                b("in", "input", json!({})),
                b("llm", "llm", json!({})),
                b("out", "output", json!({})),
            ],
            vec![
                ln("in", "out", "llm", "in"),
                ln("llm", "out", "llm", "in"),
                ln("llm", "out", "out", "in"),
            ],
        );
        l.limits.max_llm_calls = 3;
        let (o, _) = go(&l, &Fake::default(), "x").await;
        assert!(matches!(o.status, RunStatus::LimitReached { .. }));
        assert_eq!(o.llm_calls, 3);
        assert_eq!(o.replies.len(), 2);

        l.limits = RunLimits {
            max_steps: 5,
            max_llm_calls: 100,
        };
        let (o, _) = go(&l, &Fake::default(), "x").await;
        assert!(matches!(o.status, RunStatus::LimitReached { .. }));
    }

    #[tokio::test]
    async fn list_blocks_work() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("re", "regex", json!({ "pattern": "\\d+" })),
                b("each", "for_each", json!({})),
                b("pick", "separate", json!({ "outputs": 2 })),
                b("join", "join", json!({ "separator": "+" })),
                b("cap", "capture", json!({ "pattern": "(\\w)=(\\d)" })),
                b("o", "output", json!({})),
            ],
            vec![
                ln("in", "out", "re", "in"),
                ln("re", "out", "each", "in"),
                ln("re", "out", "pick", "in"),
                ln("re", "out", "join", "in"),
                ln("each", "out", "o", "in"),
                ln("pick", "out2", "o", "in"),
                ln("join", "out", "o", "in"),
                ln("in", "out", "cap", "in"),
                ln("cap", "group2", "join", "in"),
            ],
        );
        let (o, _) = go(&l, &Fake::default(), "a=1 b=2 c=3").await;
        assert_eq!(o.status, RunStatus::Completed, "{o:?}");
        assert_eq!(o.replies, vec!["1", "2", "3", "2", "1+2+3", "1+2+3"]);
    }

    #[tokio::test]
    async fn workspace_blocks_use_the_host() {
        let l = lp(
            vec![
                b("p", "fixed", json!({ "text": "notes.md" })),
                b("c", "fixed", json!({ "text": "hello" })),
                b("w", "write_file", json!({})),
                b("r", "read_file", json!({})),
                b("o", "output", json!({})),
            ],
            vec![
                ln("p", "out", "w", "path"),
                ln("c", "out", "w", "content"),
                ln("w", "out", "r", "path"),
                ln("r", "out", "o", "in"),
            ],
        );
        let host = Fake::default();
        let (o, _) = go(&l, &host, "").await;
        assert_eq!(o.replies, vec!["hello"]);

        let l = lp(
            vec![
                b("p", "fixed", json!({ "text": "missing.txt" })),
                b("r", "read_file", json!({})),
                b("o", "output", json!({})),
            ],
            vec![ln("p", "out", "r", "path"), ln("r", "out", "o", "in")],
        );
        let (o, _) = go(&l, &host, "").await;
        assert!(
            matches!(o.status, RunStatus::Failed { ref error, .. } if error.contains("not found"))
        );
    }

    #[tokio::test]
    async fn invalid_loops_do_not_run_and_waiting_is_reported() {
        let l = lp(vec![b("o", "output", json!({}))], vec![]);
        let (o, _) = go(&l, &Fake::default(), "").await;
        assert!(matches!(o.status, RunStatus::Failed { .. }));

        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("c", "combine", json!({ "inputs": 2 })),
            ],
            vec![ln("in", "out", "c", "in1")],
        );
        let (o, _) = go(&l, &Fake::default(), "").await;
        assert_eq!(o.waiting, vec!["c"]);
    }

    #[tokio::test]
    async fn history_and_overrides() {
        let l = lp(
            vec![
                b("h", "history", json!({ "max_messages": 1 })),
                b("in", "input", json!({})),
                b("o", "output", json!({})),
            ],
            vec![ln("h", "out", "o", "in"), ln("in", "out", "o", "in")],
        );
        let inputs = RunInputs {
            message: "new".into(),
            history: vec![
                ChatTurn {
                    role: "user".into(),
                    text: "old".into(),
                },
                ChatTurn {
                    role: "assistant".into(),
                    text: "reply".into(),
                },
            ],
            overrides: HashMap::from([("in".to_string(), "override".to_string())]),
        };
        let out = run(
            &l,
            &Fake::default(),
            &inputs,
            &AtomicBool::new(false),
            |_| {},
        )
        .await;
        assert_eq!(out.replies, vec!["Assistant: reply", "override"]);
    }

    #[tokio::test]
    async fn can_be_stopped() {
        let l = lp(
            vec![b("in", "input", json!({})), b("o", "output", json!({}))],
            vec![ln("in", "out", "o", "in")],
        );
        let out = run(
            &l,
            &Fake::default(),
            &RunInputs::default(),
            &AtomicBool::new(true),
            |_| {},
        )
        .await;
        assert_eq!(out.status, RunStatus::Stopped);
    }
}
