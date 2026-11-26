//! chummyd's Matrix channel end to end, against a real Synapse: chummyd
//! creates the rooms, someone verifies it by emoji (the operator confirms
//! in chummyd's terminal), then talks to agents, answers a question by
//! replying, and sends a file.
//!
//! Needs Synapse: set `CHUMMY_TEST_SYNAPSE` to a Python that has it
//! (`pip install matrix-synapse`). Skipped otherwise, and as root.

#![recursion_limit = "256"]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use chummy_bundle::{BundleAgent, Config};
use chummy_flow::{Block, Link, LoopFile, Position};
use futures_util::StreamExt;
use matrix_sdk::config::SyncSettings;
use matrix_sdk::encryption::verification::SasState;
use matrix_sdk::ruma::api::client::uiaa;
use matrix_sdk::ruma::events::room::message::{
    MessageType, OriginalSyncRoomMessageEvent, Relation, RoomMessageEventContent,
};
use matrix_sdk::ruma::{OwnedEventId, OwnedRoomId, UserId};
use matrix_sdk::{Client, Room};

const PASSWORD: &str = "a-long-test-password-for-chummyd";
const BOT_PW: &str = "bot-password-for-tests";
const ALICE_PW: &str = "alice-password-for-tests";
const WAIT: Duration = Duration::from_secs(60);

fn privileged() -> bool {
    #[cfg(unix)]
    let p = rustix::process::geteuid().is_root();
    #[cfg(windows)]
    let p = is_elevated::is_elevated();
    p
}

struct Synapse {
    child: Child,
    url: String,
    dir: tempfile::TempDir,
    python: PathBuf,
}

impl Drop for Synapse {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

impl Synapse {
    fn start(python: PathBuf) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().display();
        let port = free_port();
        let limits = "{per_second: 1000, burst_count: 1000}";
        let yaml = format!(
            "server_name: localhost
pid_file: {d}/hs.pid
listeners:
  - port: {port}
    bind_addresses: ['127.0.0.1']
    type: http
    tls: false
    resources:
      - names: [client]
        compress: false
database:
  name: sqlite3
  args:
    database: {d}/hs.db
media_store_path: {d}/media
signing_key_path: {d}/signing.key
registration_shared_secret: test-secret
report_stats: false
trusted_key_servers: []
suppress_key_server_warning: true
rc_message: {limits}
rc_login:
  address: {limits}
  account: {limits}
  failed_attempts: {limits}
rc_joins:
  local: {limits}
rc_invites:
  per_room: {limits}
  per_user: {limits}
"
        );
        let config = dir.path().join("homeserver.yaml");
        std::fs::write(&config, yaml).unwrap();
        let ok = Command::new(&python)
            .args(["-m", "synapse.app.homeserver", "--generate-keys", "-c"])
            .arg(&config)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "Synapse couldn't make its keys");
        let child = Command::new(&python)
            .args(["-m", "synapse.app.homeserver", "-c"])
            .arg(&config)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let url = format!("http://127.0.0.1:{port}");
        let me = Self {
            child,
            url,
            dir,
            python,
        };
        let deadline = Instant::now() + WAIT;
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < deadline, "Synapse didn't start");
            std::thread::sleep(Duration::from_millis(200));
        }
        // Listening isn't quite ready.
        std::thread::sleep(Duration::from_secs(1));
        me
    }

    fn register(&self, user: &str, password: &str) {
        let script = self.python.with_file_name("register_new_matrix_user");
        let out = Command::new(&self.python)
            .arg(script)
            .args(["-u", user, "-p", password, "--no-admin", "-c"])
            .arg(self.dir.path().join("homeserver.yaml"))
            .arg(&self.url)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "couldn't register {user}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
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

fn loop_file(ask: bool) -> LoopFile {
    let mut blocks = vec![
        block("in", "input", serde_json::json!({})),
        block("out", "output", serde_json::json!({})),
    ];
    let links = if ask {
        blocks.push(block(
            "ask",
            "ask_user",
            serde_json::json!({ "prompt": "Which one?" }),
        ));
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

fn agent(id: &str, name: &str, loop_id: &str) -> BundleAgent {
    BundleAgent {
        id: id.into(),
        name: name.into(),
        loop_id: loop_id.into(),
        model: None,
        tools: Default::default(),
        network: false,
        loop_config: Default::default(),
        model_options: Default::default(),
        channel_approvals: false,
    }
}

struct Daemon {
    child: Child,
    stdin: ChildStdin,
    out: Receiver<String>,
    seen: Vec<String>,
}

impl Daemon {
    fn start(bundle: &Path, password: &Path, jails: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_chummyd"))
            .args(["run", "--bundle"])
            .arg(bundle)
            .arg("--jail-dir")
            .arg(jails)
            .arg("--state")
            .arg(bundle.with_extension("state"))
            .arg("--password-file")
            .arg(password)
            .args(["--grace", "10"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let (tx, out) = channel();
        for pipe in [
            Box::new(child.stdout.take().unwrap()) as Box<dyn std::io::Read + Send>,
            Box::new(child.stderr.take().unwrap()),
        ] {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    // Shown when the test fails.
                    println!("chummyd| {line}");
                    let _ = tx.send(line);
                }
            });
        }
        Self {
            child,
            stdin,
            out,
            seen: vec![],
        }
    }

    fn send(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Waits for a line containing `text`; returns it and the lines after
    /// it that came with it.
    fn expect(&mut self, text: &str) -> Vec<String> {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if let Ok(l) = self.out.recv_timeout(Duration::from_millis(200)) {
                self.seen.push(l.clone());
                if l.contains(text) {
                    let mut lines = vec![l];
                    while let Ok(more) = self.out.recv_timeout(Duration::from_millis(300)) {
                        self.seen.push(more.clone());
                        lines.push(more);
                    }
                    return lines;
                }
            }
        }
        panic!("never saw “{text}”; saw:\n{}", self.seen.join("\n"));
    }

    fn stop(mut self) {
        self.send("/quit");
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{status}\n{}", self.seen.join("\n"));
                return;
            }
            assert!(Instant::now() < deadline, "didn't stop");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// A message Alice's app shows.
#[derive(Debug, Clone)]
struct Seen {
    room: OwnedRoomId,
    event: OwnedEventId,
    body: String,
    notice: bool,
}

struct Alice {
    client: Client,
    rx: tokio::sync::mpsc::UnboundedReceiver<Seen>,
    typing: tokio::sync::mpsc::UnboundedReceiver<(OwnedRoomId, bool)>,
    seen: Vec<Seen>,
}

impl Alice {
    async fn log_in(url: &str) -> Self {
        let client = Client::builder()
            .homeserver_url(url)
            .http_client(chummy_http::builder().build().unwrap())
            .build()
            .await
            .unwrap();
        client
            .matrix_auth()
            .login_username("@alice:localhost", ALICE_PW)
            .send()
            .await
            .unwrap();
        // Cross-signed, like any Matrix app.
        if let Err(e) = client.encryption().bootstrap_cross_signing(None).await {
            let info = e.as_uiaa_response().expect("asks for the password");
            let mut pw = uiaa::Password::new(
                uiaa::UserIdentifier::Matrix(uiaa::MatrixUserIdentifier::new("alice".into())),
                ALICE_PW.into(),
            );
            pw.session = info.session.clone();
            client
                .encryption()
                .bootstrap_cross_signing(Some(uiaa::AuthData::Password(pw)))
                .await
                .unwrap();
        }
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        client.add_event_handler(move |ev: OriginalSyncRoomMessageEvent, room: Room| {
            let tx = tx.clone();
            async move {
                if ev.sender.as_str() != "@bot:localhost" {
                    return;
                }
                // An edit shows as the new text.
                let content = match &ev.content.relates_to {
                    Some(Relation::Replacement(r)) => r.new_content.msgtype.clone(),
                    _ => ev.content.msgtype.clone(),
                };
                let (body, notice) = match content {
                    MessageType::Text(t) => (t.body, false),
                    MessageType::Notice(n) => (n.body, true),
                    _ => return,
                };
                let _ = tx.send(Seen {
                    room: room.room_id().to_owned(),
                    event: ev.event_id.clone(),
                    body,
                    notice,
                });
            }
        });
        // Whether chummyd shows it's typing, in each room.
        let (typing_tx, typing) = tokio::sync::mpsc::unbounded_channel();
        client.add_event_handler(
            move |ev: matrix_sdk::ruma::events::typing::SyncTypingEvent, room: Room| {
                let tx = typing_tx.clone();
                async move {
                    let on = ev.content.user_ids.iter().any(|u| u == "@bot:localhost");
                    let _ = tx.send((room.room_id().to_owned(), on));
                }
            },
        );
        let c = client.clone();
        tokio::spawn(async move {
            let _ = c
                .sync(SyncSettings::default().timeout(Duration::from_secs(1)))
                .await;
        });
        Self {
            client,
            rx,
            typing,
            seen: vec![],
        }
    }

    /// Waits for the bot to say something matching in `room`.
    async fn expect(&mut self, room: &OwnedRoomId, f: impl Fn(&Seen) -> bool) -> Seen {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                Ok(Some(s)) => {
                    self.seen.push(s.clone());
                    if &s.room == room && f(&s) {
                        return s;
                    }
                }
                _ => panic!("never saw it; saw: {:#?}", self.seen),
            }
        }
    }

    /// Waits until chummyd shows it's typing in `room` (or stops).
    async fn expect_typing(&mut self, room: &OwnedRoomId, on: bool) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            match tokio::time::timeout_at(deadline, self.typing.recv()).await {
                Ok(Some((r, t))) if &r == room && t == on => return,
                Ok(Some(_)) => {}
                _ => panic!(
                    "chummyd never {} typing",
                    if on { "started" } else { "stopped" }
                ),
            }
        }
    }

    /// Fails if chummyd stops typing in `room` within `time`.
    async fn typing_stays(&mut self, room: &OwnedRoomId, time: Duration) {
        let until = tokio::time::Instant::now() + time;
        while let Ok(Some((r, on))) = tokio::time::timeout_at(until, self.typing.recv()).await {
            assert!(
                &r != room || on,
                "chummyd stopped typing while the run went on"
            );
        }
    }

    fn room(&self, id: &OwnedRoomId) -> Room {
        self.client.get_room(id).unwrap()
    }

    async fn say(&self, room: &OwnedRoomId, text: &str) -> OwnedEventId {
        self.room(room)
            .send(RoomMessageEventContent::text_plain(text))
            .await
            .unwrap()
            .response
            .event_id
    }
}

async fn until<T>(what: &str, mut f: impl AsyncFnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(t) = f().await {
            return t;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn people_verify_chummyd_then_talk_to_agents_in_their_rooms() {
    let Some(python) = std::env::var_os("CHUMMY_TEST_SYNAPSE").map(PathBuf::from) else {
        eprintln!("skipped: set CHUMMY_TEST_SYNAPSE to a Python with Synapse");
        return;
    };
    if privileged() {
        eprintln!("skipped: chummyd doesn't run as root or Administrator");
        return;
    }
    let hs = Synapse::start(python);
    hs.register("bot", BOT_PW);
    hs.register("alice", ALICE_PW);

    // A model that never answers: a run that calls it goes on until it's
    // stopped.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let model_url = format!("http://{}/v1", silent.local_addr().unwrap());
    std::thread::spawn(move || {
        let mut held = vec![];
        for conn in silent.incoming().flatten() {
            held.push(conn);
        }
    });

    let tmp = tempfile::tempdir().unwrap();
    let mut c = Config::new_instance("Test", "UTC").unwrap();
    c.providers.push(
        serde_json::from_value(serde_json::json!({
            "id": "m", "name": "M", "kind": "open_ai_compatible",
            "base_url": model_url, "model": "x"
        }))
        .unwrap(),
    );
    c.loops.insert(
        "slow".into(),
        LoopFile {
            blocks: vec![
                block("in", "input", serde_json::json!({})),
                block("m", "llm", serde_json::json!({})),
                block("out", "output", serde_json::json!({})),
            ],
            links: vec![
                Link::new(("in", "out"), ("m", "in")),
                Link::new(("m", "out"), ("out", "in")),
            ],
            ..LoopFile::new("slow")
        },
    );
    c.agents.push(agent("thinker", "Thinker", "slow"));
    c.loops.insert("echo".into(), loop_file(false));
    c.loops.insert("asker".into(), loop_file(true));
    c.agents.push(agent("echoer", "Echoer", "echo"));
    c.agents.push(agent("asker", "Asker", "asker"));
    c.channels.terminal.enabled = false;
    c.channels.matrix = Some(
        serde_json::from_value(serde_json::json!({
            "homeserver": hs.url,
            "user_id": "@bot:localhost",
            "password": BOT_PW,
            "users": {
                "echoer": ["@alice:localhost"],
                "asker": ["@alice:localhost"],
                "thinker": ["@alice:localhost"],
            },
            "upload_mb": 1,
        }))
        .unwrap(),
    );
    assert!(c.check().errors.is_empty(), "{:?}", c.check().errors);
    let bundle = tmp.path().join("test.chummyd");
    std::fs::write(&bundle, chummy_bundle::seal(&c, PASSWORD).unwrap()).unwrap();
    let password = tmp.path().join("password");
    std::fs::write(
        &password,
        format!(
            "{}\n",
            *chummy_bundle::BundleKey::from_password(PASSWORD, &c.instance.id)
                .unwrap()
                .to_line()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let jails = tmp.path().join("jails");

    let mut d = Daemon::start(&bundle, &password, &jails);
    d.expect("Connected to Matrix");

    // chummyd invites Alice to a room for each agent.
    let mut alice = Alice::log_in(&hs.url).await;
    let invited = until("the invites", async || {
        let rooms = alice.client.invited_rooms();
        (rooms.len() == 3).then_some(rooms)
    })
    .await;
    for r in &invited {
        r.join().await.unwrap();
    }
    let by_name = async |name: &str| {
        until(name, async || {
            alice
                .client
                .joined_rooms()
                .into_iter()
                .find(|r| r.name().as_deref() == Some(name))
                .map(|r| r.room_id().to_owned())
        })
        .await
    };
    let echo = by_name("Echoer (Test)").await;
    let ask = by_name("Asker (Test)").await;
    let think = by_name("Thinker (Test)").await;
    assert!(
        alice
            .room(&echo)
            .latest_encryption_state()
            .await
            .unwrap()
            .is_encrypted()
    );

    // Not verified yet: nothing reaches the agent.
    alice.say(&echo, "hello?").await;
    alice
        .expect(&echo, |s| {
            s.notice && s.body.contains("Verify this session")
        })
        .await;

    // Alice verifies chummyd; the operator checks the emoji.
    let bot = UserId::parse("@bot:localhost").unwrap();
    let identity = until("the bot's identity", async || {
        alice
            .client
            .encryption()
            .request_user_identity(&bot)
            .await
            .ok()
            .flatten()
    })
    .await;
    let req = identity.request_verification().await.unwrap();
    until("chummyd to accept", async || req.is_ready().then_some(())).await;
    let sas = req.start_sas().await.unwrap().expect("emoji verification");
    let mut sas_changes = sas.changes();
    let emoji = until("the emoji", async || sas.emoji()).await;
    let shown = d.expect("wants to verify chummyd").join("\n");
    for e in &emoji {
        assert!(shown.contains(e.description), "{shown}");
    }
    assert!(shown.contains("/verify 1 yes"), "{shown}");
    d.send("/verify 1 yes");
    d.expect("Confirmed");
    sas.confirm().await.unwrap();
    let done = tokio::time::timeout(WAIT, async {
        while let Some(s) = sas_changes.next().await {
            if matches!(s, SasState::Done { .. }) {
                return;
            }
        }
    })
    .await;
    assert!(
        done.is_ok() || sas.is_done(),
        "the verification didn't finish"
    );
    d.expect("Verified @alice:localhost");

    // Now Alice talks to the agent.
    alice.say(&echo, "ping").await;
    alice
        .expect(&echo, |s| !s.notice && s.body.contains("ping"))
        .await;

    // Commands work in rooms; terminal ones don't.
    alice.say(&echo, "/status").await;
    alice
        .expect(&echo, |s| s.notice && s.body.contains("Nothing is running"))
        .await;
    alice.say(&echo, "/agents").await;
    alice
        .expect(&echo, |s| s.notice && s.body.contains("for the terminal"))
        .await;

    // The asker asks; Alice answers by replying to the question.
    // chummyd shows it's typing while a run goes (kept up past Matrix's
    // few seconds), and stops when it ends.
    alice.say(&think, "hmm").await;
    alice.expect_typing(&think, true).await;
    alice.typing_stays(&think, Duration::from_secs(12)).await;
    alice.say(&think, "/stop").await;
    alice.expect_typing(&think, false).await;

    // It stops while a question waits.
    alice.say(&ask, "start").await;
    let q = alice
        .expect(&ask, |s| s.notice && s.body.contains("Which one?"))
        .await;
    let reply = RoomMessageEventContent::text_plain("the blue one").make_reply_to(
        matrix_sdk::ruma::events::room::message::ReplyMetadata::new(&q.event, &bot, None),
        matrix_sdk::ruma::events::room::message::ForwardThread::No,
        matrix_sdk::ruma::events::room::message::AddMentions::No,
    );
    alice.room(&ask).send(reply).await.unwrap();
    alice
        .expect(&ask, |s| !s.notice && s.body.contains("the blue one"))
        .await;

    // A file goes to the agent's uploads.
    let room = alice.room(&echo);
    room.send_attachment(
        "notes/../report.txt",
        &"text/plain".parse().unwrap(),
        b"quarterly figures".to_vec(),
        Default::default(),
    )
    .await
    .unwrap();
    alice
        .expect(&echo, |s| s.notice && s.body.contains("Saved to uploads/"))
        .await;
    let saved = until("the file", async || {
        std::fs::read_dir(jails.join("echoer").join("uploads"))
            .ok()?
            .flatten()
            .next()
    })
    .await;
    assert_eq!(std::fs::read(saved.path()).unwrap(), b"quarterly figures");
    // Too large for the bundle's 1 MB.
    room.send_attachment(
        "big.bin",
        &"application/octet-stream".parse().unwrap(),
        vec![0u8; 1024 * 1024 + 1],
        Default::default(),
    )
    .await
    .unwrap();
    alice
        .expect(&echo, |s| s.notice && s.body.contains("1 MB at most"))
        .await;

    // Invites to the bot are refused.
    let other = alice
        .client
        .create_room(matrix_sdk::ruma::api::client::room::create_room::v3::Request::new())
        .await
        .unwrap();
    other.invite_user_by_id(&bot).await.unwrap();
    d.expect("Refused an invite");

    d.stop();

    // After a restart, the same rooms and the same verified session.
    let mut d = Daemon::start(&bundle, &password, &jails);
    d.expect("Connected to Matrix");
    alice.say(&echo, "again").await;
    alice
        .expect(&echo, |s| !s.notice && s.body.contains("again"))
        .await;
    assert_eq!(alice.client.joined_rooms().len(), 4, "no new rooms");
    d.stop();
}
