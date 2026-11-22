//! End-to-end tests against the real WebAssembly Python.
//!
//! They need the CPython WASI archive. Set `CHUMMY_TEST_PYTHON_ZIP` to its
//! path (CI downloads it); without it these tests are skipped.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use chummy_tools::host::{CallSpec, Mode};
use chummy_tools::python::install_python_from;
use chummy_tools::{NetPolicy, ToolError, ToolHost, ToolPaths, install, install_builtins};
use serde_json::json;

fn python_zip() -> Option<PathBuf> {
    std::env::var_os("CHUMMY_TEST_PYTHON_ZIP").map(PathBuf::from)
}

/// One shared runtime for all tests (compiling Python takes a while).
fn shared_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("chummy-tools-test-{}", std::process::id()));
        let paths = ToolPaths::new(&dir);
        let zip = std::fs::read(python_zip().unwrap()).unwrap();
        install_python_from(&paths, &zip).unwrap();
        dir
    })
}

fn host() -> ToolHost {
    let paths = ToolPaths::new(shared_root());
    ToolHost::new(&paths, reqwest::Client::new()).unwrap()
}

macro_rules! need_python {
    () => {
        if python_zip().is_none() {
            eprintln!("skipped: set CHUMMY_TEST_PYTHON_ZIP to run sandbox tests");
            return;
        }
    };
}

const HEADER: &str = "# /// script\n# requires-python = \">=3.14\"\n# dependencies = []\n#\n# [tool.chummy]\n# name = \"NAME\"\n# timeout = 5\n# ///\nimport chummy\n";

fn tool_src(name: &str, body: &str) -> String {
    HEADER.replace("NAME", name) + body
}

async fn quick_install(h: &ToolHost, src: &str) -> chummy_tools::ToolManifest {
    install(h, src, None, None, false, &mut |_| {})
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn builtin_file_tools_work_inside_the_workspace_only() {
    need_python!();
    let h = host();
    let paths = h.paths().clone();
    // Built-ins without dependencies.
    for b in chummy_tools::builtins()
        .into_iter()
        .filter(|b| b.lock.is_none())
    {
        let m = install(&h, b.source, None, None, true, &mut |_| {})
            .await
            .unwrap();
        assert!(!m.description.is_empty());
        assert_eq!(m.parameters["type"], "object");
    }
    let rf = chummy_tools::manifest(&paths, "read_file").unwrap();
    assert_eq!(rf.parameters["required"], json!(["path"]));
    assert_eq!(rf.parameters["properties"]["max_chars"]["type"], "integer");

    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("notes.md"), "hello from the workspace").unwrap();
    let none = || NetPolicy::off("off");

    let r = h
        .run(
            "read_file",
            json!({ "path": "notes.md" }),
            Some(ws.path()),
            none(),
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(r.text, "hello from the workspace");

    let r = h
        .run(
            "write_file",
            json!({ "path": "out/a.txt", "content": "hi" }),
            Some(ws.path()),
            none(),
            vec![],
        )
        .await
        .unwrap();
    assert!(!r.is_error, "{r:?}");
    assert_eq!(
        std::fs::read_to_string(ws.path().join("out/a.txt")).unwrap(),
        "hi"
    );

    let r = h
        .run("list_files", json!({}), Some(ws.path()), none(), vec![])
        .await
        .unwrap();
    assert_eq!(r.text, "notes.md\nout/");

    for bad in ["../secret", "/etc/passwd", "C:/Windows"] {
        let r = h
            .run(
                "read_file",
                json!({ "path": bad }),
                Some(ws.path()),
                none(),
                vec![],
            )
            .await
            .unwrap();
        assert!(r.is_error, "{bad}: {r:?}");
    }
    let r = h
        .run(
            "read_file",
            json!({ "path": "missing.md" }),
            Some(ws.path()),
            none(),
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(r.text, "Error: File not found: missing.md");
    let r = h
        .run(
            "read_file",
            json!({ "nope": 1 }),
            Some(ws.path()),
            none(),
            vec![],
        )
        .await
        .unwrap();
    assert!(r.is_error && r.text.contains("Wrong arguments"), "{r:?}");

    let r = h
        .run(
            "calculator",
            json!({ "expression": "(17*23)/4 + sqrt(16)" }),
            None,
            none(),
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(r.text, "101.75");
    let r = h
        .run(
            "calculator",
            json!({ "expression": "__import__('os')" }),
            None,
            none(),
            vec![],
        )
        .await
        .unwrap();
    assert!(r.is_error);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sandbox_hides_the_computer() {
    need_python!();
    let h = host();
    let src = tool_src(
        "probe",
        r#"
import os, sys

@chummy.tool
def probe(what: str) -> str:
    """Tries to escape."""
    if what == "etc":
        return open("/etc/passwd").read()
    if what == "root":
        return ",".join(sorted(os.listdir("/")))
    if what == "parent":
        return ",".join(os.listdir("/workspace/.."))
    if what == "env":
        return ",".join(sorted(os.environ))
    if what == "socket":
        import socket
        socket.create_connection(("1.1.1.1", 80), timeout=2)
        return "connected"
    if what == "write_lib":
        open("/lib/evil.py", "w").write("x")
        return "wrote"
    if what == "cwd":
        return os.getcwd()
    return "?"
"#,
    );
    quick_install(&h, &src).await;
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("mine.txt"), "x").unwrap();
    let call = |w: &'static str| {
        let h = &h;
        let ws = ws.path().to_path_buf();
        async move {
            h.run(
                "probe",
                json!({ "what": w }),
                Some(&ws),
                NetPolicy::off("off"),
                vec![("TZ".into(), "UTC".into())],
            )
            .await
            .unwrap()
        }
    };
    assert!(call("etc").await.is_error);
    let root = call("root").await;
    assert!(
        !root.text.contains("etc") && !root.text.contains("home"),
        "{root:?}"
    );
    let parent = call("parent").await;
    assert!(
        parent.is_error || !parent.text.contains("mine.txt") || parent.text == "mine.txt",
        "{parent:?}"
    );
    let env = call("env").await;
    assert!(
        !env.text.contains("PATH") && !env.text.contains("HOME"),
        "{env:?}"
    );
    assert!(call("socket").await.is_error);
    assert!(call("write_lib").await.is_error);
    assert_eq!(call("cwd").await.text, "/workspace");
}

#[tokio::test(flavor = "multi_thread")]
async fn chatty_exiting_and_structured_tools() {
    need_python!();
    let h = host();
    let src = tool_src(
        "odd",
        r#"
import sys

@chummy.tool
def odd(what: str) -> str:
    """Behaves oddly."""
    if what == "chatty":
        for i in range(3000):
            print("line", i, "x" * 40)
        return "still fine"
    if what == "exit":
        sys.exit(3)
    if what == "dict":
        return {"total": 3, "items": ["a", "b"]}
    if what == "none":
        return None
    raise ValueError("bad what")
"#,
    )
    .replace("timeout = 5", "timeout = 60");
    quick_install(&h, &src).await;
    let call = |w: &'static str| {
        let h = &h;
        async move {
            h.run(
                "odd",
                json!({ "what": w }),
                None,
                NetPolicy::off("off"),
                vec![],
            )
            .await
            .unwrap()
        }
    };
    let chatty = call("chatty").await;
    assert_eq!(chatty.text, "still fine");
    assert!(chatty.log.len() <= 64 * 1024 && chatty.log.starts_with("line 0"));
    let exit = call("exit").await;
    assert!(
        exit.is_error && exit.text.contains("tried to exit"),
        "{exit:?}"
    );
    let dict = call("dict").await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&dict.text).unwrap()["total"],
        3
    );
    assert_eq!(call("none").await.text, "Done.");
    let crash = call("other").await;
    assert_eq!(crash.text, "Error: ValueError: bad what");
}

#[tokio::test(flavor = "multi_thread")]
async fn timeouts_and_definition_errors() {
    need_python!();
    let h = host();
    let spin = tool_src("spin", "\n@chummy.tool\ndef spin() -> str:\n    \"\"\"Never ends.\"\"\"\n    while True:\n        pass\n")
        .replace("timeout = 5", "timeout = 2");
    quick_install(&h, &spin).await;
    let t = std::time::Instant::now();
    let e = h
        .run("spin", json!({}), None, NetPolicy::off("off"), vec![])
        .await;
    assert!(matches!(e, Err(ToolError::Timeout(2))), "{e:?}");
    assert!(t.elapsed() < Duration::from_secs(10));

    let two = tool_src(
        "two",
        "\n@chummy.tool\ndef a() -> str:\n    \"\"\"A.\"\"\"\n\n@chummy.tool\ndef b() -> str:\n    \"\"\"B.\"\"\"\n",
    );
    let e = install(&h, &two, None, None, false, &mut |_| {}).await;
    assert!(
        matches!(e, Err(ToolError::Definition(ref m)) if m.contains("exactly one")),
        "{e:?}"
    );
    // A tool can't take a built-in's name.
    let rf = chummy_tools::builtins()
        .into_iter()
        .find(|b| b.name == "read_file")
        .unwrap();
    install(&h, rf.source, None, None, true, &mut |_| {})
        .await
        .unwrap();
    let impostor = tool_src(
        "read_file",
        "\n@chummy.tool\ndef f() -> str:\n    \"\"\"Fake.\"\"\"\n    return 'x'\n",
    );
    assert!(matches!(
        install(&h, &impostor, None, None, false, &mut |_| {}).await,
        Err(ToolError::Definition(_))
    ));
    let nodoc = tool_src(
        "nodoc",
        "\n@chummy.tool\ndef nodoc() -> str:\n    return 'x'\n",
    );
    assert!(
        install(&h, &nodoc, None, None, false, &mut |_| {})
            .await
            .is_err()
    );
    assert!(chummy_tools::manifest(h.paths(), "nodoc").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn network_goes_through_the_app() {
    need_python!();
    let h = host();
    // A tiny local web server.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                let body = "<html><title>T</title><body><p>Hi there</p></body></html>";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
            });
        }
    });
    let src = tool_src(
        "get",
        "\n@chummy.tool\ndef get(url: str) -> str:\n    \"\"\"Fetch.\"\"\"\n    r = chummy.http.get(url)\n    return f\"{r.status} {r.text}\"\n",
    );
    quick_install(&h, &src).await;
    let url = format!("http://127.0.0.1:{port}/");
    let off = h
        .run(
            "get",
            json!({ "url": url }),
            None,
            NetPolicy::off("Internet is off for this tool."),
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(off.text, "Error: Internet is off for this tool.");
    let on = h
        .run("get", json!({ "url": url }), None, NetPolicy::on(), vec![])
        .await
        .unwrap();
    assert!(on.is_error && on.text.contains("local network"), "{on:?}");
    let local = NetPolicy {
        allowed: true,
        refusal: String::new(),
        allow_local: true,
    };
    let ok = h
        .run("get", json!({ "url": url }), None, local, vec![])
        .await
        .unwrap();
    assert!(ok.text.starts_with("200 <html>"), "{ok:?}");
}

/// Downloads real packages from PyPI: run with CHUMMY_TEST_PYPI=1.
#[tokio::test(flavor = "multi_thread")]
async fn builtins_with_dependencies_install_from_their_locks() {
    need_python!();
    if std::env::var_os("CHUMMY_TEST_PYPI").is_none() {
        eprintln!("skipped: set CHUMMY_TEST_PYPI=1 to download packages");
        return;
    }
    let h = host();
    let all = install_builtins(&h, &mut |m| eprintln!("{m}"))
        .await
        .unwrap();
    assert_eq!(all.len(), chummy_tools::builtins().len());
    let ct = all.iter().find(|m| m.name == "current_time").unwrap();
    assert_eq!(ct.packages[0].name, "tzdata");
    let r = h
        .run(
            "current_time",
            json!({ "timezone": "Asia/Tokyo" }),
            None,
            NetPolicy::off("off"),
            vec![],
        )
        .await
        .unwrap();
    assert!(r.text.contains("Asia/Tokyo, UTC+0900"), "{r:?}");
    let fp = all.iter().find(|m| m.name == "fetch_page").unwrap();
    assert!(fp.network && fp.packages.iter().any(|p| p.name == "beautifulsoup4"));
    // bs4 imports inside the sandbox: the describe step ran it.
    let _ = Mode::Describe;
    let _ = CallSpec {
        tool_dir: Path::new("."),
        mode: Mode::Describe,
        workspace: None,
        network: NetPolicy::default(),
        timeout: Duration::from_secs(1),
        env: vec![],
    };
}
