//! chummyd with a Discord bot, against a stand-in Discord: the operator
//! makes an invitation code and lets someone in at the terminal, they talk
//! to the agent in direct messages, and they're still let in after chummyd
//! starts again.

#[path = "../../channel/tests/support/fake_discord.rs"]
mod fake;

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use chummy_bundle::{BundleAgent, Config};
use chummy_flow::{Block, Link, LoopFile, Position};
use fake::{Fake, TOKEN, dm, user};
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
    });
    c.channels.list.push(
        serde_json::from_value(json!({
            "id": "phone",
            "name": "Phone",
            "type": "discord",
            "agents": ["echoer"],
            "options": { "api_url": url, "upload_mb": 25, "stream": true },
            "secrets": { "token": TOKEN.as_str() },
        }))
        .unwrap(),
    );
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

/// Waits for the bot's answer in Alice's direct messages: posted, or an
/// earlier message edited into it.
async fn answered(fake: &Fake, starts: &str) {
    let path = format!("/api/v10/channels/{}/messages", dm("42"));
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let mut all = fake.calls("POST", &path);
        all.extend(fake.calls("PATCH", &path));
        if all
            .iter()
            .any(|b| b["content"].as_str().unwrap_or("").starts_with(starts))
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no answer starting “{starts}”: {:?}", fake.sent_to("42"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn people_let_in_at_the_terminal_talk_to_the_agent() {
    if privileged() {
        return;
    }
    let fake = Fake::start().await;
    let (_tmp, bundle, password) = setup(&fake.url);
    let alice = user("42", "alice");

    let mut d = Daemon::start(&bundle, &password);
    d.expect("[Phone] People reach Echoer as @helper_bot.");
    d.send("/people");
    d.expect("[Phone] Nobody yet.");
    d.send("/invite");
    let line = d.expect("[Phone] Invitation code:");
    let code = line.rsplit(' ').next().unwrap().to_string();
    assert_eq!(code.len(), 6, "{line}");
    fake.say(&alice, &code);
    d.expect("Alice (@alice), Discord id 42 sent the invitation code");
    d.expect("/verify 1 yes (let them in)");
    d.send("/verify yes");
    fake.wait_call(
        "POST",
        &format!("/api/v10/channels/{}/messages", dm("42")),
        |b| {
            b["content"]
                .as_str()
                .unwrap_or("")
                .contains("can talk to Echoer now")
        },
    )
    .await;

    fake.say(&alice, "hello from Discord");
    answered(&fake, "hello from Discord\n\n").await;
    // In a server, nothing happens.
    fake.in_server(&alice, "hello from a server");
    d.send("/people");
    d.expect("[Phone] Alice (@alice) (id 42)");
    let seen = d.quit();
    assert!(
        !seen
            .iter()
            .any(|l| l.contains(TOKEN.as_str()) || l.contains("fakefake")),
        "the token was printed"
    );
    assert!(
        !fake
            .sent_to("42")
            .iter()
            .any(|t| t.starts_with("hello from a server"))
    );

    // Started again: Alice is still let in.
    let mut d = Daemon::start(&bundle, &password);
    d.expect("[Phone] People reach Echoer as @helper_bot.");
    fake.say(&alice, "still here");
    answered(&fake, "still here\n\n").await;
    d.send("/remove 42");
    d.expect("[Phone] 42 can't talk to Echoer there any more.");
    d.quit();
    let sealed = std::fs::read(bundle.with_file_name("test.chummyd.state/state.sealed")).unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains("alice"));
}
