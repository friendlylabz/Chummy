//! The chummyd image (docker/chummyd/Dockerfile) end to end: it runs
//! locked down as in the docs (read-only, no capabilities, no network),
//! takes its password as a secret file, talks in the terminal, stops
//! cleanly on `docker stop` and keeps its state across containers.
//!
//! Needs Docker and the image: set `CHUMMY_TEST_IMAGE` to its name
//! (`docker build -f docker/chummyd/Dockerfile -t chummyd .`, then
//! `CHUMMY_TEST_IMAGE=chummyd`). Skipped otherwise, and as root.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use chummy_bundle::{BundleAgent, Config};
use chummy_flow::{Block, Link, LoopFile, Position};

const PASSWORD: &str = "a-long-test-password-for-chummyd";
const WAIT: Duration = Duration::from_secs(60);

fn block(id: &str, kind: &str) -> Block {
    Block {
        id: id.into(),
        kind: kind.into(),
        label: id.into(),
        position: Position::default(),
        params: serde_json::json!({}),
        comment: String::new(),
    }
}

fn bundle() -> Config {
    let mut c = Config::new_instance("Docker", "UTC").unwrap();
    c.providers.push(
        serde_json::from_value(serde_json::json!({
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
    c.channels.terminal.enabled = true;
    c
}

struct Container {
    name: String,
    child: Child,
    stdin: ChildStdin,
    out: Receiver<String>,
    seen: Vec<String>,
}

impl Container {
    fn start(image: &str, name: &str, dir: &Path) -> Self {
        let uid = rustix::process::getuid().as_raw();
        let gid = rustix::process::getgid().as_raw();
        let mount = |from: &str, to: &str, ro: bool| {
            format!(
                "type=bind,src={},dst={to}{}",
                dir.join(from).display(),
                if ro { ",readonly" } else { "" }
            )
        };
        let mut child = Command::new("docker")
            .args(["run", "--rm", "-i", "--name", name])
            // The files here are the test's own; the image's user would
            // be 65532.
            .args(["--user", &format!("{uid}:{gid}")])
            .args(["--read-only", "--tmpfs", "/tmp", "--cap-drop", "ALL"])
            .args([
                "--security-opt",
                "no-new-privileges:true",
                "--network",
                "none",
            ])
            .args(["--mount", &mount("bundle", "/bundle", true)])
            .args([
                "--mount",
                &mount("password", "/run/secrets/chummyd_password", true),
            ])
            .args(["--mount", &mount("jails", "/jails", false)])
            .args(["--mount", &mount("state", "/state", false)])
            .arg(image)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("docker runs");
        let stdin = child.stdin.take().unwrap();
        let (tx, out) = channel();
        for pipe in [
            Box::new(child.stdout.take().unwrap()) as Box<dyn std::io::Read + Send>,
            Box::new(child.stderr.take().unwrap()),
        ] {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    println!("chummyd| {line}");
                    let _ = tx.send(line);
                }
            });
        }
        Self {
            name: name.into(),
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

    fn expect(&mut self, text: &str) {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if let Ok(l) = self.out.recv_timeout(Duration::from_millis(200)) {
                let hit = l.contains(text);
                self.seen.push(l);
                if hit {
                    return;
                }
            }
        }
        panic!("never saw “{text}”; saw:\n{}", self.seen.join("\n"));
    }

    /// `docker stop`: SIGTERM, as Compose and orchestrators send.
    fn stop(mut self) {
        let ok = Command::new("docker")
            .args(["stop", "--time", "30", &self.name])
            .stdout(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "docker stop failed");
        let status = self.child.wait().unwrap();
        assert!(status.success(), "{status}\n{}", self.seen.join("\n"));
        self.expect("chummyd stopped");
    }
}

#[test]
fn the_image_runs_locked_down_and_keeps_its_state() {
    let Ok(image) = std::env::var("CHUMMY_TEST_IMAGE") else {
        eprintln!("skipped: set CHUMMY_TEST_IMAGE to a chummyd image");
        return;
    };
    if rustix::process::geteuid().is_root() {
        eprintln!("skipped: chummyd doesn't run as root");
        return;
    }

    // What the image promises.
    let inspect = |what: &str| {
        let out = Command::new("docker")
            .args(["image", "inspect", "--format", what, &image])
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    assert_eq!(inspect("{{.Config.User}}"), "65532:65532");
    assert_eq!(inspect("{{json .Config.ExposedPorts}}"), "null");
    let version = Command::new("docker")
        .args(["run", "--rm", "--network", "none", &image, "--version"])
        .output()
        .unwrap();
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).contains("chummyd"));
    // No shell to break into.
    let shell = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--entrypoint",
            "/bin/sh",
            &image,
            "-c",
            "true",
        ])
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!shell.success(), "the image has a shell");

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    for d in ["bundle", "jails", "state"] {
        std::fs::create_dir(dir.join(d)).unwrap();
    }
    let config = bundle();
    std::fs::write(
        dir.join("bundle").join("bundle.chummyd"),
        chummy_bundle::seal(&config, PASSWORD).unwrap(),
    )
    .unwrap();
    // The password's hash, as chummyd-docker-init writes it.
    let password = dir.join("password");
    std::fs::write(
        &password,
        format!(
            "{}\n",
            *chummy_bundle::BundleKey::from_password(PASSWORD, &config.instance.id)
                .unwrap()
                .to_line()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o400)).unwrap();
    }
    let name = format!("chummyd-test-{}", std::process::id());

    let mut c = Container::start(&image, &name, dir);
    c.expect("Talking to Echoer");
    c.send("hello from docker");
    c.expect("Echoer: hello from docker");
    c.stop();
    assert!(dir.join("state").join("state.sealed").is_file());
    assert!(dir.join("jails").join("agents").join("echoer").is_dir());

    // A new container carries on with the same state.
    let mut c = Container::start(&image, &name, dir);
    c.expect("Talking to Echoer");
    c.send("/status");
    c.expect("Nothing is running");
    c.stop();
}
