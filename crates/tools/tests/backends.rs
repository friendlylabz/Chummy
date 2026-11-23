//! The Container and OpenShell sandboxes, end to end.
//!
//! - Container: runs only when `CHUMMY_TEST_DOCKER_IMAGE` names a local
//!   image with Python 3.14 and `sh` (CI pulls `python:3.14-slim` on Linux).
//! - OpenShell: runs against `fake_openshell.py`, a stand-in for the
//!   `openshell` program that keeps each "sandbox" in a folder and runs
//!   commands with the host's Python. It checks Chummy's side: the
//!   arguments, the policy, copying in and out, and the line protocol.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chummy_sandbox::{Jail, Protected};
use chummy_tools::host::Mode;
use chummy_tools::sandbox::{
    ContainerOptions, ContainerSandbox, OpenShellOptions, OpenShellSandbox, SandboxBackend,
    SandboxCall, SandboxKind,
};
use chummy_tools::{NetPolicy, ToolError, ToolPaths};
use serde_json::{Value, json};

const PROBE: &str = r#"# /// script
# dependencies = []
#
# [tool.chummy]
# name = "probe"
# needs = ["programs"]
# ///
"""A test tool that looks around its sandbox."""

import os
import socket
import subprocess
import time

import chummy


@chummy.tool
def probe(action: str, url: str = "") -> str:
    """Probe the sandbox.

    action: files, net, programs or sleep
    url: for net (for files: a host path that must not be visible)
    """
    if action == "files":
        with open("made.txt", "w") as f:
            f.write("made")
        with open("keep.txt") as f:
            old = f.read()
        with open("keep.txt", "w") as f:
            f.write(old + "+changed")
        os.remove("gone.txt")
        os.makedirs("sub/deeper", exist_ok=True)
        with open("sub/deeper/x.txt", "w") as f:
            f.write("x")
        try:
            os.symlink("/etc/passwd", "planted")
        except OSError:
            pass
        return "|".join([
            "ws=" + chummy.WORKSPACE,
            "cwd=" + os.getcwd(),
            "host_home_visible=" + str(os.path.exists(url)),
            "env_leak=" + str("CARGO_MANIFEST_DIR" in os.environ),
        ])
    if action == "net":
        try:
            socket.create_connection(("1.1.1.1", 53), timeout=3).close()
            direct = "connected"
        except OSError:
            direct = "blocked"
        r = chummy.http.get(url)
        return f"direct={direct}|bridge={r.status}:{r.text}"
    if action == "programs":
        out = subprocess.run(["sh", "-c", "echo $((6*7))"], capture_output=True, text=True)
        return out.stdout.strip()
    if action == "sleep":
        time.sleep(60)
        return "woke"
    raise chummy.ToolError("unknown action " + action)
"#;

/// Tool paths with the runner written and the probe tool "installed".
fn paths() -> (tempfile::TempDir, ToolPaths, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let paths = ToolPaths::new(root.path());
    chummy_tools::python::write_support(&paths).unwrap();
    let tool = paths.tool_dir("probe");
    std::fs::create_dir_all(tool.join("site-packages")).unwrap();
    std::fs::write(tool.join("tool.py"), PROBE).unwrap();
    (root, paths, tool)
}

fn workspace() -> (tempfile::TempDir, Jail) {
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("keep.txt"), "kept").unwrap();
    std::fs::write(ws.path().join("gone.txt"), "bye").unwrap();
    let jail = Jail::open(ws.path(), &Protected::none()).unwrap();
    (ws, jail)
}

async fn call(
    b: &dyn SandboxBackend,
    tool: &Path,
    jail: Option<&Jail>,
    args: Value,
    network: NetPolicy,
    timeout: u64,
) -> Result<Result<Value, (String, String)>, ToolError> {
    b.call(SandboxCall {
        tool_dir: tool,
        mode: Mode::Call(args),
        jail,
        network,
        timeout: Duration::from_secs(timeout),
        env: vec![("TZ".into(), "UTC".into())],
    })
    .await
    .map(|o| o.result)
}

/// A one-shot local web server (tests allow local addresses).
async fn web_server() -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut s, _)) = l.accept().await {
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf).await;
            let _ = s
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello",
                )
                .await;
        }
    });
    format!("http://{addr}/")
}

fn local_net() -> NetPolicy {
    NetPolicy {
        allowed: true,
        refusal: String::new(),
        allow_local: true,
    }
}

fn text(r: Result<Value, (String, String)>) -> String {
    match r {
        Ok(Value::String(s)) => s,
        other => panic!("unexpected result: {other:?}"),
    }
}

/// The files test, shared by both backends.
/// `isolated`: the backend really hides the host (the fake OpenShell
/// doesn't).
fn check_files(ws: &Path, out: &str, expect_ws: &str, isolated: bool) {
    assert!(out.contains(&format!("ws={expect_ws}")), "{out}");
    assert!(out.contains(&format!("cwd={expect_ws}")), "{out}");
    if isolated {
        assert!(out.contains("host_home_visible=False"), "{out}");
    }
    assert!(out.contains("env_leak=False"), "{out}");
    assert_eq!(
        std::fs::read_to_string(ws.join("made.txt")).unwrap(),
        "made"
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("keep.txt")).unwrap(),
        "kept+changed"
    );
    assert!(!ws.join("gone.txt").exists());
    assert_eq!(
        std::fs::read_to_string(ws.join("sub/deeper/x.txt")).unwrap(),
        "x"
    );
}

// ------------------------------------------------------------- container

#[tokio::test]
async fn container_runs_tools_locked_down() {
    let Ok(image) = std::env::var("CHUMMY_TEST_DOCKER_IMAGE") else {
        eprintln!("skipping: set CHUMMY_TEST_DOCKER_IMAGE to a local image with Python 3.14");
        return;
    };
    // Cargo sets CARGO_MANIFEST_DIR for test runs: it must not leak in.
    assert!(std::env::var_os("CARGO_MANIFEST_DIR").is_some());
    let (_root, paths, tool) = paths();
    let mut opts = ContainerOptions {
        image,
        ..Default::default()
    };
    let probe = ContainerSandbox::new(opts.clone(), paths.clone()).unwrap();
    opts.approved_id = Some(
        probe
            .get_image(false)
            .await
            .expect("image on this computer"),
    );
    let b = ContainerSandbox::new(opts, paths.clone()).unwrap();
    assert_eq!(b.kind(), SandboxKind::Container);

    let h = b.check().await;
    assert!(h.ready, "{h:?}");
    assert!(h.python.as_deref().unwrap().starts_with("3.14"), "{h:?}");
    assert!(h.programs);

    // Files: the workspace is mounted at /workspace and nothing else is.
    let (ws, jail) = workspace();
    let out = text(
        call(
            &b,
            &tool,
            Some(&jail),
            json!({"action": "files", "url": env!("CARGO_MANIFEST_DIR")}),
            NetPolicy::off("no"),
            30,
        )
        .await
        .unwrap(),
    );
    check_files(ws.path(), &out, "/workspace", true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let owner = std::fs::metadata(ws.path()).unwrap().uid();
        assert_eq!(
            std::fs::metadata(ws.path().join("made.txt")).unwrap().uid(),
            owner
        );
    }

    // No network of its own; chummy.http goes through Chummy.
    let url = web_server().await;
    let out = text(
        call(
            &b,
            &tool,
            Some(&jail),
            json!({"action": "net", "url": url}),
            local_net(),
            30,
        )
        .await
        .unwrap(),
    );
    assert_eq!(out, "direct=blocked|bridge=200:hello");
    // …and Chummy's policy still decides.
    let refused = call(
        &b,
        &tool,
        Some(&jail),
        json!({"action": "net", "url": url}),
        NetPolicy::off("Internet access is off."),
        30,
    )
    .await
    .unwrap();
    assert!(refused.unwrap_err().1.contains("Internet access is off"));

    // Real programs work here.
    let out = text(
        call(
            &b,
            &tool,
            Some(&jail),
            json!({"action": "programs"}),
            NetPolicy::off("no"),
            30,
        )
        .await
        .unwrap(),
    );
    assert_eq!(out, "42");

    // The real-shell built-in.
    let shell = paths.tool_dir("run_shell");
    std::fs::create_dir_all(shell.join("site-packages")).unwrap();
    std::fs::write(
        shell.join("tool.py"),
        include_str!("../builtin/run_shell.py"),
    )
    .unwrap();
    let out = text(
        call(
            &b,
            &shell,
            Some(&jail),
            json!({"command": format!(
                "echo hi; cat keep.txt; echo; test -e '{}' && echo host-visible || echo host-hidden; exit 3",
                env!("CARGO_MANIFEST_DIR")
            )}),
            NetPolicy::off("no"),
            30,
        )
        .await
        .unwrap(),
    );
    assert!(out.starts_with("hi\nkept+changed"), "{out}");
    assert!(out.contains("host-hidden"), "the host isn't visible: {out}");
    assert!(out.ends_with("[exit code 3]"), "{out}");

    // A call that runs too long is stopped, and its container is gone.
    let err = call(
        &b,
        &tool,
        Some(&jail),
        json!({"action": "sleep"}),
        NetPolicy::off("no"),
        1,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ToolError::Timeout(1)), "{err:?}");
    tokio::time::sleep(Duration::from_secs(2)).await;
    let left = std::process::Command::new("docker")
        .args([
            "ps",
            "-aq",
            "--filter",
            "label=com.friendlylabz.chummy=tool",
        ])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&left.stdout).trim().is_empty(),
        "containers left behind"
    );
}

#[tokio::test]
async fn container_without_an_image_explains() {
    let (_root, paths, tool) = paths();
    let b = ContainerSandbox::new(ContainerOptions::default(), paths).unwrap();
    let err = call(
        &b,
        &tool,
        None,
        json!({"action": "programs"}),
        NetPolicy::off("no"),
        5,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Security and Privacy"), "{err}");
}

// ------------------------------------------------------------- openshell

const FAKE: &str = include_str!("fake_openshell.py");

fn fake_openshell(dir: &Path) -> Option<PathBuf> {
    if cfg!(windows) || which_python().is_none() {
        return None;
    }
    let script = dir.join("openshell");
    let py = which_python()?;
    std::fs::write(&script, format!("#!{}\n{FAKE}", py.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    Some(script)
}

fn which_python() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join("python3"))
            .find(|p| p.is_file())
    })
}

#[tokio::test]
async fn openshell_copies_the_workspace_in_and_out() {
    let fake_dir = tempfile::tempdir().unwrap();
    let Some(program) = fake_openshell(fake_dir.path()) else {
        eprintln!("skipping: needs python3 on a Unix system");
        return;
    };
    let state = fake_dir.path().join("state");
    // The fake reads this from its environment, which Chummy cleans; pass
    // it through a file next to the script instead.
    std::fs::write(
        fake_dir.path().join("state-dir"),
        state.display().to_string(),
    )
    .unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let (_root, paths, tool) = paths();
    let b = OpenShellSandbox::new(
        OpenShellOptions {
            program: program.display().to_string(),
            image: "ghcr.io/example/py:3".into(),
        },
        paths,
        scratch.path().to_path_buf(),
    )
    .unwrap();

    let h = b.check().await;
    assert!(h.ready, "{h:?}");
    assert!(h.programs && h.python.is_some(), "{h:?}");

    let (ws, jail) = workspace();
    let out = call(
        &b,
        &tool,
        Some(&jail),
        json!({"action": "files", "url": env!("CARGO_MANIFEST_DIR")}),
        NetPolicy::off("no"),
        30,
    )
    .await
    .unwrap();
    let out = text(out);
    let name = chummy_tools::sandbox::openshell::sandbox_name(Some(jail.root()));
    // The workspace copy is /sandbox/<call id>/workspace (the fake shows
    // where it keeps /sandbox).
    let ws_seen = out
        .split('|')
        .next()
        .unwrap()
        .trim_start_matches("ws=")
        .to_string();
    assert!(
        ws_seen.contains("/sandbox/call-") && ws_seen.ends_with("/workspace"),
        "{out}"
    );
    check_files(ws.path(), &out, &ws_seen, false);
    // The link the tool planted didn't come back.
    assert!(std::fs::symlink_metadata(ws.path().join("planted")).is_err());

    // The bridge works through `sandbox exec`.
    let url = web_server().await;
    let out = text(
        call(
            &b,
            &tool,
            Some(&jail),
            json!({"action": "net", "url": url}),
            local_net(),
            30,
        )
        .await
        .unwrap(),
    );
    assert!(out.ends_with("|bridge=200:hello"), "{out}");

    // What the fake saw: one sandbox per workspace, made once, with
    // Chummy's policy, and each call's folder removed afterwards.
    let log = std::fs::read_to_string(state.join("calls.log")).unwrap();
    let creates: Vec<&str> = log
        .lines()
        .filter(|l| l.starts_with("sandbox create"))
        .collect();
    assert_eq!(
        creates.len(),
        2,
        "check sandbox + workspace sandbox:\n{log}"
    );
    assert!(creates.iter().any(|l| l.contains(&name)), "{log}");
    for c in &creates {
        for flag in [
            "--policy",
            "--detach",
            "--no-tty",
            "--no-auto-providers",
            "--label chummy=tools",
            "--from ghcr.io/example/py:3",
        ] {
            assert!(c.contains(flag), "missing {flag}: {c}");
        }
    }
    let policy = std::fs::read_to_string(state.join("last-policy.yaml")).unwrap();
    assert!(policy.contains("hard_requirement"));
    assert!(
        log.lines().any(|l| l.contains("rm -rf /sandbox/call-")),
        "{log}"
    );
    let remote = state.join(&name).join("sandbox");
    let left: Vec<_> = std::fs::read_dir(&remote).unwrap().collect();
    assert!(
        left.is_empty(),
        "call folders left in the sandbox: {left:?}"
    );
    // Nothing staged is left in Chummy's scratch folder either.
    let staged: Vec<_> = std::fs::read_dir(scratch.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("call-"))
        .collect();
    assert!(staged.is_empty());

    // Remove Chummy's sandboxes.
    assert_eq!(b.remove_all().await.unwrap(), 2);
}

#[tokio::test]
async fn openshell_explains_a_missing_gateway() {
    let fake_dir = tempfile::tempdir().unwrap();
    let Some(program) = fake_openshell(fake_dir.path()) else {
        return;
    };
    std::fs::write(
        fake_dir.path().join("state-dir"),
        fake_dir.path().join("s").display().to_string(),
    )
    .unwrap();
    std::fs::write(fake_dir.path().join("gateway-down"), "").unwrap();
    let (_root, paths, _tool) = paths();
    let b = OpenShellSandbox::new(
        OpenShellOptions {
            program: program.display().to_string(),
            image: String::new(),
        },
        paths,
        fake_dir.path().join("scratch"),
    )
    .unwrap();
    let h = b.check().await;
    assert!(!h.ready);
    assert!(h.message.contains("gateway isn't reachable"), "{h:?}");
}
