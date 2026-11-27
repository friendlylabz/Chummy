//! chummyd end to end: a bundle made the way the app makes it, run with the
//! terminal channel through pipes. The loops need no model.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use chummy_bundle::{BundleAgent, Config};
use chummy_flow::{Block, Link, LoopFile, Position};

const PASSWORD: &str = "a-long-test-password-for-chummyd";

/// chummyd refuses to run as root or Administrator, so these tests only run
/// as an ordinary user (as CI does).
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

/// message → (a question) → reply.
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
        loop_config: Default::default(),
        model_options: Default::default(),
        channel_approvals: false,
    }
}

struct Setup {
    _tmp: tempfile::TempDir,
    bundle: PathBuf,
    password: PathBuf,
    jails: PathBuf,
}

/// The temporary folder may itself be off-limits to agents (inside
/// Windows' AppData, say): then chummyd rightly refuses it as a jail dir.
fn temp_is_usable(tmp: &Path) -> bool {
    let ok = chummy_sandbox::validate_workspace(
        tmp,
        &chummy_sandbox::Protected::for_current_user::<&Path>(&[]),
    )
    .is_ok();
    if !ok {
        eprintln!("skipped: the temporary folder can't hold agents' folders here");
    }
    ok
}

fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let mut c = Config::new_instance("Test", "UTC").unwrap();
    c.providers.push(
        serde_json::from_value(serde_json::json!({
            "id": "m", "name": "M", "kind": "open_ai_compatible",
            "base_url": "http://127.0.0.1:9/v1", "model": "x"
        }))
        .unwrap(),
    );
    c.loops.insert("echo".into(), loop_file(false));
    c.loops.insert("asker".into(), loop_file(true));
    c.agents.push(agent("echoer", "Echoer", "echo"));
    c.agents.push(agent("asker", "Asker", "asker"));
    c.channels.terminal.enabled = true;
    let bundle = tmp.path().join("test.chummyd");
    std::fs::write(&bundle, chummy_bundle::seal(&c, PASSWORD).unwrap()).unwrap();
    let password = tmp.path().join("password");
    // The password's hash, as chummyd-docker-init writes it.
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
    Setup {
        jails: tmp.path().join("jails"),
        _tmp: tmp,
        bundle,
        password,
    }
}

struct Daemon {
    child: Child,
    stdin: ChildStdin,
    out: Receiver<String>,
    seen: Vec<String>,
}

impl Daemon {
    fn start(s: &Setup) -> Self {
        Self::start_with(s, &s.password)
    }

    fn start_with(s: &Setup, password: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_chummyd"))
            .args(["run", "--bundle"])
            .arg(&s.bundle)
            .arg("--state")
            .arg(s.bundle.with_file_name("test.chummyd.state"))
            .arg("--jail-dir")
            .arg(&s.jails)
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

    /// Waits for a line containing `text`.
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

    fn quit(mut self) {
        self.send("/quit");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{status}");
                return;
            }
            assert!(Instant::now() < deadline, "didn't stop");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[test]
fn a_conversation_carries_on_across_restarts() {
    if privileged() {
        return;
    }
    let s = setup();
    if !temp_is_usable(s.bundle.parent().unwrap()) {
        return;
    }
    let mut d = Daemon::start(&s);
    d.expect("Talking to Echoer");
    d.send("hello there");
    d.expect("Echoer: hello there");
    d.expect("· ");
    // Commands never reach the agent.
    d.send("/nonsense");
    d.expect("Unknown command: /nonsense");
    d.send("/agents");
    d.expect("Asker [asker]");
    d.quit();
    assert!(s.jails.join("echoer").is_dir() && s.jails.join("asker").is_dir());
    let sealed = std::fs::read(s.bundle.with_file_name("test.chummyd.state/state.sealed")).unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains("hello there"));

    // The same conversation, after starting again.
    let mut d = Daemon::start(&s);
    d.expect("Talking to Echoer");
    d.send("/status");
    d.expect("Nothing is running");
    d.quit();
}

#[test]
fn a_run_paused_by_shutting_down_waits_for_resume() {
    if privileged() {
        return;
    }
    let s = setup();
    if !temp_is_usable(s.bundle.parent().unwrap()) {
        return;
    }
    let mut d = Daemon::start(&s);
    d.expect("Talking to Echoer");
    d.send("/agent asker");
    d.expect("Now talking to Asker");
    d.send("the report");
    d.expect("Asker: Which one?");
    d.quit();

    let mut d = Daemon::start(&s);
    d.send("/agent asker");
    d.expect("has a paused run");
    // Not carried on by itself, and new messages wait for it.
    d.send("something else");
    d.expect("paused run");
    d.send("/resume");
    d.expect("Carrying on");
    d.expect("Asker: Which one?");
    d.send("the second one");
    d.expect("Asker: the second one");
    d.quit();
}

#[test]
fn a_wrong_password_or_a_jail_dir_holding_the_bundle_stops_it() {
    if privileged() {
        return;
    }
    let s = setup();
    if !temp_is_usable(s.bundle.parent().unwrap()) {
        return;
    }
    let instance = chummy_bundle::header(&std::fs::read(&s.bundle).unwrap())
        .unwrap()
        .instance_id;
    let bad = s.password.with_file_name("bad");
    let run_with = |file: &Path| {
        Command::new(env!("CARGO_BIN_EXE_chummyd"))
            .args(["run", "--bundle"])
            .arg(&s.bundle)
            .arg("--state")
            .arg(s.bundle.with_file_name("test.chummyd.state"))
            .arg("--jail-dir")
            .arg(&s.jails)
            .arg("--password-file")
            .arg(file)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    // The hash of a wrong password.
    let wrong =
        chummy_bundle::BundleKey::from_password("not the right password", &instance).unwrap();
    std::fs::write(&bad, format!("{}\n", *wrong.to_line())).unwrap();
    let out = run_with(&bad);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("password is wrong"));
    // The password itself: never taken from a file.
    std::fs::write(&bad, format!("{PASSWORD}\n")).unwrap();
    let out = run_with(&bad);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("never the password"));

    // hash-password: the password on standard input, the line out.
    let hash = |password: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_chummyd"))
            .args(["hash-password", "--bundle"])
            .arg(&s.bundle)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(child.stdin.take().unwrap(), "{password}").unwrap();
        child.wait_with_output().unwrap()
    };
    let out = hash(PASSWORD);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        std::fs::read_to_string(&s.password).unwrap()
    );
    let out = hash("not the right password");
    assert!(!out.status.success() && out.stdout.is_empty());

    let out = Command::new(env!("CARGO_BIN_EXE_chummyd"))
        .args(["run", "--bundle"])
        .arg(&s.bundle)
        .arg("--state")
        .arg(s.bundle.with_file_name("test.chummyd.state"))
        .arg("--jail-dir")
        .arg(s.bundle.parent().unwrap())
        .arg("--password-file")
        .arg(&s.password)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("overlap"));

    // A state folder from somewhere else doesn't open: it stops, and the
    // folder is left alone.
    let state = s.bundle.with_file_name("test.chummyd.state");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(
        state.join("state.sealed"),
        b"CHUMMYD-STATE\x01not really a state file at all..........",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_chummyd"))
        .args(["run", "--bundle"])
        .arg(&s.bundle)
        .arg("--state")
        .arg(s.bundle.with_file_name("test.chummyd.state"))
        .arg("--jail-dir")
        .arg(&s.jails)
        .arg("--password-file")
        .arg(&s.password)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("won't start"));
    assert!(state.join("state.sealed").exists());
}
