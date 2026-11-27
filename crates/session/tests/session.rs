//! A whole session against a test platform: files for chats, memory for
//! paused runs, loops that need no model. What's checked here holds for
//! every program that runs agents through `chummy-session`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chummy_agent::{Agent, AgentConfig, Permissions, Workspace};
use chummy_flow::{Answer, Block, Link, LoopFile, Position};
use chummy_llm::ProviderConfig;
use chummy_log::runs::LogOptions;
use chummy_runner::Services;
use chummy_runner::usage::UsageLedger;
use chummy_sandbox::{Jail, Protected};
use chummy_scheduler::{Cause, Job, RecordStatus, RunRequest};
use chummy_session::{
    ChatStore, FileChats, LoopProblem, PausedOpenError, PausedStore, Platform, Session, Update,
    chats,
};
use chummy_tools::{Health, SandboxBackend, ToolManifest, ToolPaths};
use serde_json::json;
use zeroize::Zeroizing;

struct NoServices;

impl Services for NoServices {
    fn secret(&self, _: &str) -> Option<String> {
        None
    }
    async fn sandbox(&self) -> Result<Arc<dyn SandboxBackend>, String> {
        Err("no tools here".into())
    }
    fn tool_paths(&self) -> ToolPaths {
        ToolPaths::new("/nonexistent")
    }
    async fn local_engine(&self, _: &ProviderConfig) -> Result<(String, String), String> {
        Err("no engine".into())
    }
}

#[derive(Clone)]
struct Config {
    agents: Vec<Agent>,
    providers: Vec<ProviderConfig>,
    permissions: Permissions,
}

impl AgentConfig for Config {
    fn agents(&self) -> &[Agent] {
        &self.agents
    }
    fn providers(&self) -> &[ProviderConfig] {
        &self.providers
    }
    fn default_provider(&self) -> Option<&str> {
        None
    }
    fn permissions(&self) -> &Permissions {
        &self.permissions
    }
}

#[derive(Default)]
struct MemoryPaused(Mutex<HashMap<(String, String), Vec<u8>>>);

impl PausedStore for MemoryPaused {
    fn seal(&self, agent: &str, chat: &str, bytes: &[u8]) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .insert((agent.into(), chat.into()), bytes.to_vec());
        Ok(())
    }
    fn open(&self, agent: &str, chat: &str) -> Result<Option<Zeroizing<Vec<u8>>>, PausedOpenError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(&(agent.into(), chat.into()))
            .cloned()
            .map(Zeroizing::new))
    }
    fn take(&self, agent: &str, chat: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .remove(&(agent.into(), chat.into()))
            .map(Zeroizing::new))
    }
    fn discard(&self, agent: &str, chat: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(&(agent.into(), chat.into()));
        Ok(())
    }
    fn discard_agent(&self, agent: &str) {
        self.0.lock().unwrap().retain(|(a, _), _| a != agent);
    }
    fn list(&self) -> Vec<(String, String)> {
        self.0.lock().unwrap().keys().cloned().collect()
    }
}

struct Inner {
    session: Session,
    config: Config,
    chats: FileChats,
    paused: MemoryPaused,
    loop_file: LoopFile,
    updates: Mutex<Vec<Update>>,
    _dir: tempfile::TempDir,
}

#[derive(Clone)]
struct TestPlatform(Arc<Inner>);

impl Platform for TestPlatform {
    type Services = NoServices;
    type Config = Config;

    fn session(&self) -> &Session {
        &self.0.session
    }
    fn config(&self) -> Config {
        self.0.config.clone()
    }
    fn chats(&self) -> &dyn ChatStore {
        &self.0.chats
    }
    fn paused_store(&self) -> &dyn PausedStore {
        &self.0.paused
    }
    fn runnable_loop(&self, id: &str) -> Result<LoopFile, LoopProblem> {
        match id {
            "loop" => Ok(self.0.loop_file.clone()),
            "untrusted" => Err(LoopProblem::Untrusted("Someone's loop".into())),
            _ => Err(LoopProblem::Missing),
        }
    }
    fn open_jail(&self, path: &Path) -> Result<Jail, String> {
        Jail::open(path, &Protected::none()).map_err(|e| e.to_string())
    }
    fn services(&self) -> Arc<NoServices> {
        Arc::new(NoServices)
    }
    fn installed_tools(&self) -> Vec<ToolManifest> {
        vec![]
    }
    fn sandbox_health(&self, _: &Config) -> Option<Health> {
        None
    }
    fn time_zone(&self) -> Option<String> {
        None
    }
    fn run_logs(&self) -> Option<(PathBuf, LogOptions)> {
        None
    }
    fn spawn(&self, task: impl std::future::Future<Output = ()> + Send + 'static) {
        tokio::spawn(task);
    }
    fn emit(&self, update: Update) {
        self.0.updates.lock().unwrap().push(update);
    }
}

fn block(id: &str, kind: &str, params: serde_json::Value) -> Block {
    Block {
        id: id.into(),
        kind: kind.into(),
        label: id.into(),
        position: Position::default(),
        params,
        comment: String::new(),
    }
}

/// message → (a question, if `ask`) → reply. No model is called.
fn echo(ask: bool) -> LoopFile {
    let mut blocks = vec![
        block("in", "input", json!({})),
        block("out", "output", json!({})),
    ];
    let links = if ask {
        blocks.push(block("ask", "ask_user", json!({ "prompt": "Which one?" })));
        vec![
            Link::new(("in", "out"), ("ask", "in")),
            Link::new(("ask", "out"), ("out", "in")),
        ]
    } else {
        vec![Link::new(("in", "out"), ("out", "in"))]
    };
    LoopFile {
        blocks,
        links,
        ..LoopFile::new("echo")
    }
}

fn platform(loop_file: LoopFile) -> TestPlatform {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let provider: ProviderConfig = serde_json::from_value(json!({
        "id": "m", "name": "M", "kind": "open_ai_compatible",
        "base_url": "http://127.0.0.1:9/v1", "model": "x"
    }))
    .unwrap();
    let agent = Agent {
        id: "a1".into(),
        name: "Helper".into(),
        loop_id: "loop".into(),
        model: None,
        workspace: Workspace {
            path: ws,
            confirmed_at: 1,
        },
        created_at: 1,
        tools: Default::default(),
        loop_config: Default::default(),
        model_options: Default::default(),
    };
    TestPlatform(Arc::new(Inner {
        session: Session::new(Arc::new(UsageLedger::open(&dir.path().join("usage.json")))),
        config: Config {
            agents: vec![agent],
            providers: vec![provider],
            permissions: Permissions::default(),
        },
        chats: FileChats::new(dir.path().join("chats")),
        paused: MemoryPaused::default(),
        loop_file,
        updates: Mutex::new(vec![]),
        _dir: dir,
    }))
}

/// Waits until `f` holds, or fails the test after a few seconds.
async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..400 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

fn finished(p: &TestPlatform, run_id: &str) -> usize {
    p.0.updates
        .lock()
        .unwrap()
        .iter()
        .filter(|u| matches!(u, Update::RunFinished(f) if f.run_id == run_id))
        .count()
}

fn chat(p: &TestPlatform) -> String {
    p.chats().index("a1").unwrap().active.unwrap()
}

#[tokio::test]
async fn a_message_gets_one_response() {
    let p = platform(echo(false));
    let c = chat(&p);
    let run = chummy_session::send_message(&p, "a1", &c, "  hello  ").unwrap();
    until("the run to finish", || finished(&p, &run) == 1).await;
    let msgs = p.chats().load("a1", &c).unwrap();
    let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, ["user", "assistant"]);
    assert_eq!(
        (msgs[0].text.as_str(), msgs[1].text.as_str()),
        ("hello", "hello")
    );
    let summary = msgs[1].run.as_ref().unwrap();
    assert_eq!(
        (summary.run_id.as_str(), summary.status.as_str()),
        (run.as_str(), "completed")
    );
    // The chat is named from its first message, and the run is done.
    assert_eq!(p.chats().index("a1").unwrap().chats[0].title, "hello");
    assert!(p.session().runs().is_empty() && !p.session().chat_busy(&c));
    // Its first reply is announced (the app may name the chat with it).
    assert!(
        p.0.updates
            .lock()
            .unwrap()
            .iter()
            .any(|u| matches!(u, Update::FirstReply(f) if f.reply == "hello"))
    );
    // An empty message isn't sent.
    assert!(chummy_session::send_message(&p, "a1", &c, "  ").is_err());
}

#[tokio::test]
async fn finished_runs_are_kept_three_to_an_agent() {
    let p = platform(echo(false));
    let c = chat(&p);
    let mut ids = vec![];
    for i in 0..4 {
        let run = chummy_session::send_message(&p, "a1", &c, &format!("m{i}")).unwrap();
        until("the run to end", || {
            p.session().recent().iter().any(|r| r.info.run_id == run)
        })
        .await;
        ids.push(run);
    }
    let recent = p.session().recent();
    let got: Vec<&str> = recent.iter().map(|r| r.info.run_id.as_str()).collect();
    assert_eq!(got, [ids[3].as_str(), ids[2].as_str(), ids[1].as_str()]);
    assert!(recent.iter().all(|r| r.status == "completed"));
    assert_eq!(recent[0].info.agent_id.as_deref(), Some("a1"));
}

#[tokio::test]
async fn a_paused_run_carries_on_as_one_response() {
    let p = platform(echo(true));
    let c = chat(&p);
    let run = chummy_session::send_message(&p, "a1", &c, "the report").unwrap();
    until("the question", || !p.session().asks.pending().is_empty()).await;

    // Paused while it waits on the user: the question is cut short and
    // asked again on resuming.
    chummy_session::pause_run(&p, &run).unwrap();
    until("the pause", || finished(&p, &run) == 1).await;
    assert!(p.session().is_paused("a1", &c));
    // Paused runs aren't among the recent ones: they're paused runs.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(!p.session().recent().iter().any(|r| r.info.run_id == run));
    assert!(p.session().asks.pending().is_empty(), "withdrawn");
    let msgs = p.chats().load("a1", &c).unwrap();
    assert_eq!(msgs.last().unwrap().role, "notice");
    assert_eq!(msgs.last().unwrap().run.as_ref().unwrap().status, "paused");
    // A paused chat takes no new message until it's resumed or discarded.
    assert!(chummy_session::send_message(&p, "a1", &c, "more").is_err());

    // Resumed: the same run, the note gone, the question asked again.
    let again = chummy_session::resume_run(&p, "a1", &c).unwrap();
    assert_eq!(again, run);
    assert!(!p.session().is_paused("a1", &c));
    assert!(p.paused_store().list().is_empty(), "taken once");
    until("the question again", || {
        !p.session().asks.pending().is_empty()
    })
    .await;
    let q = p.session().asks.pending()[0].id.clone();
    chummy_session::answer(&p, &q, Answer::Text("the second".into())).unwrap();
    until("the run to finish", || finished(&p, &run) == 2).await;
    // Finished: among the recent runs once.
    until("the run among the recent ones", || {
        p.session().recent().iter().any(|r| r.info.run_id == run)
    })
    .await;
    let recent = p.session().recent();
    assert_eq!(recent.iter().filter(|r| r.info.run_id == run).count(), 1);
    assert_eq!(recent[0].status, "completed");

    let msgs = p.chats().load("a1", &c).unwrap();
    let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, ["user", "question", "answer", "assistant"]);
    assert_eq!(msgs[3].text, "the second");
    for m in &msgs[1..] {
        let r = m.run.as_ref().unwrap();
        assert_eq!(
            (r.run_id.as_str(), r.status.as_str()),
            (run.as_str(), "completed")
        );
    }
    // Nothing to resume twice.
    assert!(chummy_session::resume_run(&p, "a1", &c).is_err());
}

#[tokio::test]
async fn a_kept_run_that_doesnt_open_is_removed_with_a_note() {
    let p = platform(echo(false));
    let c = chat(&p);
    p.paused_store().seal("a1", &c, b"not a run").unwrap();
    p.paused_store().seal("gone", "c-1", b"whatever").unwrap();
    chummy_session::paused::load(&p);
    assert!(p.session().paused().is_empty());
    assert!(p.paused_store().list().is_empty());
    let msgs = p.chats().load("a1", &c).unwrap();
    assert!(
        msgs[0]
            .text
            .starts_with("The paused run couldn't be opened")
    );
}

#[tokio::test]
async fn only_trusted_loops_with_a_model_run() {
    let p = platform(echo(false));
    let mut agent = p.config().agents[0].clone();
    agent.loop_id = "untrusted".into();
    let c = p.config();
    let problem = |agent: &Agent, job_loop: Option<&str>| {
        chummy_session::prepare(&p, &c, agent, job_loop)
            .err()
            .expect("refused")
    };
    let e = problem(&agent, None);
    assert!(e.contains("hasn't been reviewed"), "{e}");
    agent.loop_id = "nope".into();
    let e = problem(&agent, None);
    assert!(e.contains("loop is missing"), "{e}");
    let e = problem(&agent, Some("nope"));
    assert!(e.contains("Choose another for the job"), "{e}");
    agent.loop_id = "loop".into();
    assert!(chummy_session::prepare(&p, &c, &agent, None).is_ok());
}

#[tokio::test]
async fn a_job_runs_as_its_agent_beside_its_chats() {
    let p = platform(echo(false));
    assert!(p.session().job_may_start("a1"));
    let mut job = Job::new("j", "Digest", "a1");
    job.message = "the news".into();
    let req = RunRequest {
        run_id: "run-job".into(),
        job,
        cause: Cause::Manual,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let r = chummy_session::run_job(&p, &req).await;
    assert_eq!(r.status, RecordStatus::Completed);
    assert_eq!(r.reply, "the news");
    assert_eq!(finished(&p, "run-job"), 1);
    // Jobs don't write to chats.
    let c = chat(&p);
    assert!(p.chats().load("a1", &c).unwrap().is_empty());
    assert_eq!(
        chats::NEW_CHAT,
        p.chats().index("a1").unwrap().chats[0].title
    );
}

#[tokio::test]
async fn shutting_down_pauses_chat_runs_and_refuses_new_ones() {
    let p = platform(echo(true));
    let c = chat(&p);
    let run = chummy_session::send_message(&p, "a1", &c, "the report").unwrap();
    until("the question", || !p.session().asks.pending().is_empty()).await;
    assert_eq!(p.session().pause_all_for_shutdown(), 1);
    until("the pause", || finished(&p, &run) == 1).await;
    assert!(p.session().is_paused("a1", &c));
    assert!(p.session().shutting_down());
    let e = chummy_session::resume_run(&p, "a1", &c).unwrap_err();
    assert!(e.contains("shutting down"), "{e}");
    let e = chummy_session::send_message(&p, "a1", &c, "more").unwrap_err();
    assert!(e.contains("shutting down"), "{e}");
}
