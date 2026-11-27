//! chummyd with a Telegram bot, against a stand-in Bot API: the operator
//! makes a pairing code and lets someone in at the terminal, they talk to
//! the agent, and they're still let in after chummyd starts again.

#[path = "../../channel/tests/support/fake_telegram.rs"]
mod fake;

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use chummy_bundle::{BundleAgent, Config, TelegramBot};
use chummy_flow::{Block, Link, LoopFile, Position};
use fake::{Fake, TOKEN, user};
use serde_json::json;

const PASSWORD: &str = "a-long-test-password-for-chummyd";

fn privileged() -> bool {
    #[cfg(unix)]
    let p = rustix::process::geteuid().is_root();
    #[cfg(windows)]
    let p = is_elevated::is_elevated();
    if p {
        eprintln!("skipped: chummyd doesn't run as root or Administrator");
    }
    p
}

fn block(id: &str, kind: &str) -> Block {
    Block {
        id: id.into(),
        kind: kind.into(),
        label: id.into(),
        position: Position::default(),
        params: json!({}),
        comment: String::new(),
    }
}

fn setup(url: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let mut c = Config::new_instance("Test", "UTC").unwrap();
    // The loop needs no model, but every agent has one.
    c.providers.push(
        serde_json::from_value(json!({
            "id": "m", "name": "M", "kind": "open_ai_compatible",
            "base_url": "http://127.0.0.1:9/v1", "model": "x"
        }))
        .unwrap(),
    );
    c.loops.insert(
        "echo".into(),
        LoopFile {
            blocks: vec![block("in", "input"), block("out", "output")],
            links: vec![Link::new(("in", "out"), ("out", "in"))],
            ..LoopFile::new("echo")
        },
    );
    c.agents.push(BundleAgent {
        id: "echoer".into(),
        name: "Echoer".into(),
        loop_id: "echo".into(),
        model: None,
        tools: Default::default(),
        loop_config: Default::default(),
        model_options: Default::default(),
        channel_approvals: false,
    });
    c.channels.telegram.push(TelegramBot {
        agent: "echoer".into(),
        token: TOKEN.into(),
        api_url: url.into(),
        upload_mb: 20,
        stream: true,
    });
    assert!(c.check().ok(), "{:?}", c.check());
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
    (tmp, bundle, password)
}

struct Daemon {
    child: Child,
    stdin: ChildStdin,
    out: Receiver<String>,
    seen: Vec<String>,
}

impl Daemon {
    fn start(bundle: &PathBuf, password: &PathBuf) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_chummyd"))
            .args(["run", "--bundle"])
            .arg(bundle)
            .arg("--state")
            .arg(bundle.with_file_name("test.chummyd.state"))
            .arg("--jail-dir")
            .arg(bundle.with_file_name("jails"))
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

    fn expect(&mut self, text: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok(l) = self.out.recv_timeout(Duration::from_millis(200)) {
                self.seen.push(l.clone());
                if l.contains(text) {
                    return l;
                }
            }
        }
        panic!("never saw “{text}”; saw:\n{}", self.seen.join("\n"));
    }

    fn quit(mut self) -> Vec<String> {
        self.send("/quit");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{status}");
                while let Ok(l) = self.out.recv_timeout(Duration::from_millis(200)) {
                    self.seen.push(l);
                }
                return self.seen;
            }
            assert!(Instant::now() < deadline, "didn't stop");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn people_paired_at_the_terminal_talk_to_the_agent() {
    if privileged() {
        return;
    }
    let fake = Fake::start().await;
    let (_tmp, bundle, password) = setup(&fake.url);
    let alice = user(42, "Alice", "alice");

    let mut d = Daemon::start(&bundle, &password);
    d.expect("Echoer's Telegram bot is @helper_bot");
    d.send("/people");
    d.expect("Nobody may talk to Echoer's Telegram bot yet");
    d.send("/pair");
    let line = d.expect("Pairing code for Echoer's Telegram bot:");
    let code = line.rsplit(' ').next().unwrap().to_string();
    assert_eq!(code.len(), 6, "{line}");
    fake.say(&alice, &format!("/start {code}"));
    d.expect("Alice (@alice), Telegram id 42 sent the pairing code");
    d.send("/pair yes");
    fake.wait_call("sendMessage", |b| {
        b["chat_id"] == json!(42)
            && b["text"]
                .as_str()
                .unwrap()
                .contains("can talk to Echoer now")
    })
    .await;

    fake.say(&alice, "hello from Telegram");
    fake.wait_call("sendMessage", |b| {
        b["chat_id"] == json!(42)
            && b["text"]
                .as_str()
                .unwrap()
                .starts_with("hello from Telegram\n\n")
    })
    .await;
    d.send("/people");
    d.expect("Alice (@alice) (id 42)");
    let seen = d.quit();
    assert!(
        !seen
            .iter()
            .any(|l| l.contains(TOKEN) || l.contains("AAHdq")),
        "the token was printed"
    );

    // Started again: Alice is still let in.
    let mut d = Daemon::start(&bundle, &password);
    d.expect("Echoer's Telegram bot is @helper_bot");
    fake.say(&alice, "still here");
    fake.wait_call("sendMessage", |b| {
        b["chat_id"] == json!(42) && b["text"].as_str().unwrap().starts_with("still here\n\n")
    })
    .await;
    d.send("/unpair 42");
    d.expect("42 can't talk to Echoer's Telegram bot any more");
    d.quit();
    let sealed = std::fs::read(bundle.with_file_name("test.chummyd.state/state.sealed")).unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains("Alice"));
}
