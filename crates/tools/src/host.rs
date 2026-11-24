//! Running tools: WebAssembly Python under Wasmtime.
//!
//! Each call gets a fresh WebAssembly instance that can see exactly:
//!
//! | In the sandbox | What it is                              | Access     |
//! |----------------|-----------------------------------------|------------|
//! | `/workspace`   | the agent's workspace (if there is one) | read/write |
//! | `/tool`        | the tool file and its packages          | read-only  |
//! | `/lib`         | Python's standard library               | read-only  |
//! | `/chummy`      | Chummy's runner and helper module       | read-only  |
//!
//! Nothing else on the computer exists for it. WASI preview 1 has no
//! sockets, so a tool has no network of its own; `chummy.http` asks the app
//! over stdin/stdout, and the app refuses unless the tool's internet access
//! is on. The host's environment variables aren't passed in. Calls have a
//! time limit and a memory limit.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use wasmtime::{
    Cache, CacheConfig, Config, Engine, InstancePre, Linker, Module, Store, StoreLimits,
    StoreLimitsBuilder, UpdateDeadline,
};
use wasmtime_wasi::cli::{IsTerminal, StdinStream, StdoutStream};
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi::{FsPerms, WasiCtxBuilder};

use crate::ToolError;
use crate::net::{NetPolicy, http_request};
use crate::python::{ToolPaths, python_status, write_support};

/// Memory a tool may use.
const MEMORY_LIMIT: usize = 512 * 1024 * 1024;
/// Most bytes a tool may write to stdout (protocol) in one call.
pub(crate) const STDOUT_LIMIT: usize = 16 * 1024 * 1024;
/// Most bytes of log (stderr) kept per call.
pub(crate) const LOG_LIMIT: usize = 64 * 1024;
/// Largest result accepted from a tool.
pub const RESULT_LIMIT: usize = 1024 * 1024;
const TICK: Duration = Duration::from_millis(50);

struct Guest {
    wasi: WasiP1Ctx,
    limits: StoreLimits,
}

/// Compiles Python once and runs tool calls.
pub struct ToolHost {
    engine: Engine,
    pre: InstancePre<Guest>,
    paths: ToolPaths,
    /// For package downloads.
    http: reqwest::Client,
    /// For tools' web requests: never follows redirects by itself, so every
    /// hop is checked (see [`crate::net`]).
    web: reqwest::Client,
}

/// What a call should do.
#[derive(Debug, Clone)]
pub enum Mode {
    /// Load the tool and report its description and parameters.
    Describe,
    /// Call the tool with these arguments.
    Call(Json),
}

#[derive(Debug, Clone)]
pub struct CallSpec<'a> {
    /// The installed tool's folder (tool.py and site-packages).
    pub tool_dir: &'a Path,
    pub mode: Mode,
    /// The agent's workspace, mounted read/write. `None`: no workspace.
    pub workspace: Option<&'a Path>,
    pub network: NetPolicy,
    pub timeout: Duration,
    /// Extra environment variables (e.g. `TZ`). The host's own environment
    /// is never passed through.
    pub env: Vec<(String, String)>,
}

/// How a call ended, when the sandbox itself worked.
#[derive(Debug, Clone, PartialEq)]
pub struct CallOutcome {
    /// The tool's result, or (kind, message) if it reported an error.
    /// Kinds: "tool" (a ToolError), "arguments", "crash", "definition".
    pub result: Result<Json, (String, String)>,
    /// What the tool printed, trimmed.
    pub log: String,
}

impl ToolHost {
    /// Compiles the WebAssembly Python (slow the first time, then cached on
    /// disk). Blocking: call it off the async runtime. `http` is used for
    /// package downloads.
    pub fn new(paths: &ToolPaths, http: reqwest::Client) -> Result<Self, ToolError> {
        if python_status(paths).is_none() {
            return Err(ToolError::Runtime(
                "the tool runtime isn't installed yet".into(),
            ));
        }
        write_support(paths)?;
        let rt = |e: wasmtime::Error| ToolError::Runtime(e.to_string());

        let mut cfg = Config::new();
        cfg.epoch_interruption(true);
        let mut cache = CacheConfig::new();
        cache.with_directory(paths.cache.join("wasmtime"));
        if let Ok(c) = Cache::new(cache) {
            cfg.cache(Some(c));
        }
        let engine = Engine::new(&cfg).map_err(rt)?;
        let module = Module::from_file(&engine, paths.wasm()).map_err(rt)?;
        let mut linker: Linker<Guest> = Linker::new(&engine);
        wasmtime_wasi::p1::add_to_linker_async(&mut linker, |g: &mut Guest| &mut g.wasi)
            .map_err(rt)?;
        let pre = linker.instantiate_pre(&module).map_err(rt)?;

        let weak = engine.weak();
        std::thread::Builder::new()
            .name("chummy-tool-clock".into())
            .spawn(move || {
                while let Some(e) = weak.upgrade() {
                    e.increment_epoch();
                    drop(e);
                    std::thread::sleep(TICK);
                }
            })
            .map_err(ToolError::io("starting the tool clock"))?;

        let web = web_client()?;
        Ok(Self {
            engine,
            pre,
            paths: paths.clone(),
            http,
            web,
        })
    }

    pub fn paths(&self) -> &ToolPaths {
        &self.paths
    }

    pub fn http_client(&self) -> reqwest::Client {
        self.http.clone()
    }

    /// Runs one call in a fresh sandbox.
    pub async fn call(&self, spec: CallSpec<'_>) -> Result<CallOutcome, ToolError> {
        let request = request_json(&spec.mode);

        let (to_guest, guest_in) = mpsc::unbounded_channel::<Vec<u8>>();
        let (guest_out, mut from_guest) = mpsc::unbounded_channel::<Vec<u8>>();
        let _ = to_guest.send((request.to_string() + "\n").into_bytes());
        let log = LogOut::default();

        let mut b = WasiCtxBuilder::new();
        b.args(&["python", "-S", "-E", "-B", "-u", "/chummy/_runner.py"])
            .stdin(ChannelIn::new(guest_in))
            .stdout(ChannelOut::new(guest_out))
            .stderr(log.clone());
        for (k, v) in &spec.env {
            b.env(k, v);
        }
        let pre =
            |e: wasmtime::Error| ToolError::Runtime(format!("couldn't prepare the sandbox: {e}"));
        b.preopened_dir(self.paths.lib(), "/lib", FsPerms::ReadOnly)
            .map_err(pre)?;
        b.preopened_dir(&self.paths.support, "/chummy", FsPerms::ReadOnly)
            .map_err(pre)?;
        b.preopened_dir(spec.tool_dir, "/tool", FsPerms::ReadOnly)
            .map_err(pre)?;
        if let Some(ws) = spec.workspace {
            b.preopened_dir(ws, "/workspace", FsPerms::ReadWrite)
                .map_err(pre)?;
        }
        let wasi = b.build_p1();
        let limits = StoreLimitsBuilder::new()
            .memory_size(MEMORY_LIMIT)
            .instances(4)
            .build();
        let mut store = Store::new(&self.engine, Guest { wasi, limits });
        store.limiter(|g| &mut g.limits);
        let deadline = Instant::now() + spec.timeout;
        store.epoch_deadline_callback(move |_| {
            if Instant::now() >= deadline {
                Ok(UpdateDeadline::Interrupt)
            } else {
                Ok(UpdateDeadline::Yield(1))
            }
        });
        store.set_epoch_deadline(1);

        // The bridge answers the tool's requests while it runs.
        let http = self.web.clone();
        let policy = spec.network.clone();
        let bridge = tokio::spawn(async move {
            let mut buf: Vec<u8> = Vec::new();
            let mut done: Option<Json> = None;
            while let Some(chunk) = from_guest.recv().await {
                buf.extend_from_slice(&chunk);
                while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=nl).collect();
                    let Ok(msg) = serde_json::from_slice::<Json>(&line) else {
                        continue;
                    };
                    if msg.get("op").and_then(|o| o.as_str()) == Some("done") {
                        done = Some(msg);
                    } else {
                        let reply = answer(&http, &policy, &msg).await;
                        let _ = to_guest.send((reply.to_string() + "\n").into_bytes());
                    }
                }
            }
            done
        });

        let started = Instant::now();
        let run = async {
            let instance = self.pre.instantiate_async(&mut store).await?;
            let start = instance.get_typed_func::<(), ()>(&mut store, "_start")?;
            start.call_async(&mut store, ()).await
        };
        // The epoch deadline only stops Python while it runs; a tool blocked
        // reading stdin (waiting in the host) needs a wall-clock limit too.
        let limit = spec.timeout + Duration::from_secs(5);
        let Ok(result) = tokio::time::timeout(limit, run).await else {
            drop(store);
            bridge.abort();
            return Err(ToolError::Timeout(spec.timeout.as_secs() as u32));
        };
        // Dropping the store closes stdout, which ends the bridge.
        drop(store);
        let done = bridge.await.ok().flatten();
        let log = log.text();

        if let Err(e) = result {
            // A normal exit(0) is reported as an error by WASI; check for it.
            let exit = e.downcast_ref::<wasmtime_wasi::I32Exit>().map(|x| x.0);
            if exit != Some(0) || done.is_none() {
                if started.elapsed() >= spec.timeout {
                    return Err(ToolError::Timeout(spec.timeout.as_secs() as u32));
                }
                if done.is_none() {
                    let why = log
                        .lines()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| short_trap(&e));
                    return Err(ToolError::Crashed(why));
                }
            }
        }
        let result = parse_done(done)?;
        Ok(CallOutcome { result, log })
    }

    /// The installed tool folder for `name`.
    pub fn tool_dir(&self, name: &str) -> PathBuf {
        self.paths.tool_dir(name)
    }
}

/// The first line a tool reads: what to do.
pub(crate) fn request_json(mode: &Mode) -> Json {
    match mode {
        Mode::Describe => json!({ "mode": "describe" }),
        Mode::Call(args) => json!({ "mode": "call", "args": args }),
    }
}

/// Answers one request a running tool sent (anything but `done`): web
/// requests go through `policy`; anything else is refused.
pub(crate) async fn answer(http: &reqwest::Client, policy: &NetPolicy, msg: &Json) -> Json {
    let id = msg.get("id").cloned().unwrap_or(Json::Null);
    match msg.get("op").and_then(|o| o.as_str()) {
        Some("http") => match http_request(http, policy, msg).await {
            Ok(mut r) => {
                r["ok"] = json!(true);
                r["id"] = id;
                r
            }
            Err(e) => json!({ "id": id, "ok": false, "error": e }),
        },
        _ => json!({ "id": id, "ok": false, "error": "unknown request" }),
    }
}

/// Reads the tool's final `done` message.
pub(crate) fn parse_done(done: Option<Json>) -> Result<Result<Json, (String, String)>, ToolError> {
    let done = done.ok_or_else(|| ToolError::Crashed("the tool didn't report a result".into()))?;
    Ok(if done.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        let r = done.get("result").cloned().unwrap_or(Json::Null);
        if r.to_string().len() > RESULT_LIMIT {
            return Err(ToolError::Crashed("the tool's result is too large".into()));
        }
        Ok(r)
    } else {
        Err((
            done.get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .to_string(),
            done.get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("The tool failed.")
                .to_string(),
        ))
    })
}

/// The client for tools' web requests: it never follows redirects by
/// itself, so every hop is checked (see [`crate::net`]).
pub(crate) fn web_client() -> Result<reqwest::Client, ToolError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .user_agent(concat!("Chummy/", env!("CARGO_PKG_VERSION"), " (tool)"))
        .build()
        .map_err(|e| ToolError::Runtime(e.to_string()))
}

fn short_trap(e: &wasmtime::Error) -> String {
    let s = e.to_string();
    if s.contains("all fuel consumed") || s.contains("interrupt") {
        "the tool was stopped".into()
    } else if s.contains("memory") {
        "the tool ran out of memory".into()
    } else {
        s.lines().next().unwrap_or("the tool crashed").to_string()
    }
}

// ------------------------------------------------------------ guest stdio

/// Guest stdin, fed from a channel.
#[derive(Clone)]
struct ChannelIn(Arc<Mutex<InState>>);

struct InState {
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    pending: Vec<u8>,
}

impl ChannelIn {
    fn new(rx: mpsc::UnboundedReceiver<Vec<u8>>) -> Self {
        Self(Arc::new(Mutex::new(InState {
            rx,
            pending: Vec::new(),
        })))
    }
}

impl AsyncRead for ChannelIn {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if s.pending.is_empty() {
            match s.rx.poll_recv(cx) {
                Poll::Ready(Some(bytes)) => s.pending = bytes,
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
        let n = s.pending.len().min(buf.remaining());
        buf.put_slice(&s.pending[..n]);
        s.pending.drain(..n);
        Poll::Ready(Ok(()))
    }
}

impl IsTerminal for ChannelIn {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdinStream for ChannelIn {
    fn async_stream(&self) -> Box<dyn AsyncRead + Send + Sync> {
        Box::new(self.clone())
    }
}

/// Guest stderr (the tool's log): keeps the first [`LOG_LIMIT`] bytes and
/// quietly drops the rest, so a chatty tool never fails on `print`.
#[derive(Clone, Default)]
struct LogOut(Arc<Mutex<Vec<u8>>>);

impl LogOut {
    fn text(&self) -> String {
        let b = self.0.lock().unwrap_or_else(|e| e.into_inner());
        String::from_utf8_lossy(&b).trim().to_string()
    }
}

impl AsyncWrite for LogOut {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut b = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let room = LOG_LIMIT.saturating_sub(b.len());
        b.extend_from_slice(&buf[..buf.len().min(room)]);
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl IsTerminal for LogOut {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdoutStream for LogOut {
    fn async_stream(&self) -> Box<dyn AsyncWrite + Send + Sync> {
        Box::new(self.clone())
    }
}

/// Guest stdout, sent to a channel, with a size limit.
#[derive(Clone)]
struct ChannelOut {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    written: Arc<AtomicUsize>,
}

impl ChannelOut {
    fn new(tx: mpsc::UnboundedSender<Vec<u8>>) -> Self {
        Self {
            tx,
            written: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl AsyncWrite for ChannelOut {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let total = self.written.fetch_add(buf.len(), Ordering::Relaxed) + buf.len();
        if total > STDOUT_LIMIT || self.tx.send(buf.to_vec()).is_err() {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl IsTerminal for ChannelOut {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdoutStream for ChannelOut {
    fn async_stream(&self) -> Box<dyn AsyncWrite + Send + Sync> {
        Box::new(self.clone())
    }
}
