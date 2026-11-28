//! The container sandbox: tool calls run in hardened Docker or Podman
//! containers.
//!
//! Chummy never installs Docker or Podman and never downloads an image on
//! its own: the user picks an image and gets it in Security and Privacy,
//! which records the image's ID. Containers run that exact ID (`--pull
//! never`), so a tag that changes later can't swap in a different image
//! unseen.
//!
//! **One container kept per workspace** (so, per agent), started at its
//! first call and removed when Chummy quits; each call is a `docker exec`
//! in it. Its only writable folder is the workspace (and a small `/tmp` in
//! memory). `HOME` is `.chummy-home` in the workspace, so what programs keep
//! there (git settings, `pip install --user`, `cargo install`, caches)
//! stays between calls; the folder ignores itself for git. Nothing a call
//! starts outlives it: the runner makes the call its own process group and
//! stops whatever is left of it when the call ends (or Chummy stops it when
//! the time is up). Calls with a direct network get a second container of
//! their own, so an offline call never shares a network. A container whose
//! settings no longer match (another image, other limits) is replaced.
//! Calls with no workspace (describing a tool) use a fresh `docker run
//! --rm -i` each time.
//!
//! Every container has:
//!
//! - `--network none`: no network at all. `chummy.http` still works, over
//!   stdin/stdout, through Chummy's own checks. The one exception is a
//!   tool that needs `direct-network` (Run shell command) with its internet
//!   access on: that call gets the engine's ordinary network, so the
//!   programs it starts can connect anywhere, unchecked (the user is told
//!   so when turning it on).
//! - only three mounts: the workspace (read/write) at `/workspace`, the
//!   installed tools at `/tools` (a fresh call: just its tool, at `/tool`)
//!   and Chummy's runner at `/chummy`, both read-only. The Docker socket is
//!   never mounted.
//! - `--read-only` root filesystem with a small `/tmp`, `--cap-drop ALL`,
//!   `--security-opt no-new-privileges`, `--ipc none`, limits on memory,
//!   CPUs and processes, no logs kept by the engine, and (with Docker on
//!   Linux and macOS) the user id of the workspace's owner rather than root.
//! - no host environment: only the variables Chummy passes (`TZ`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::process::{self, Run, clean_command};
use super::{
    BoxFut, Health, PYTHON_PROBE, SandboxBackend, SandboxCall, SandboxKind, find_program,
    last_line, parse_python,
};
use crate::ToolError;
use crate::host::{CallOutcome, web_client};
use crate::python::ToolPaths;

/// The container engine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerEngine {
    #[default]
    Docker,
    Podman,
}

impl ContainerEngine {
    pub fn program(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Docker => "Docker",
            Self::Podman => "Podman",
        }
    }
}

/// The default image: the official Python image, small variant. Tools need
/// Python 3.14 or newer for the built-ins.
pub const DEFAULT_IMAGE: &str = "python:3.14-slim";

/// Container settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContainerOptions {
    pub engine: ContainerEngine,
    /// Where the engine's program is. Empty: find it.
    pub program: String,
    /// The image the user chose (a name like `python:3.14-slim`).
    pub image: String,
    /// The ID of that image when the user got it (`sha256:…`). Calls run
    /// exactly this. `None`: not got yet.
    pub approved_id: Option<String>,
    /// Memory per call, in MB.
    pub memory_mb: u32,
    /// CPUs per call.
    pub cpus: f64,
}

impl Default for ContainerOptions {
    fn default() -> Self {
        Self {
            engine: ContainerEngine::Docker,
            program: String::new(),
            image: DEFAULT_IMAGE.into(),
            approved_id: None,
            memory_mb: 1024,
            cpus: 2.0,
        }
    }
}

impl ContainerOptions {
    /// Clamps limits into sensible ranges.
    pub fn normalized(mut self) -> Self {
        self.memory_mb = self.memory_mb.clamp(128, 65_536);
        if !self.cpus.is_finite() {
            self.cpus = 2.0;
        }
        self.cpus = self.cpus.clamp(0.25, 64.0);
        self.image = self.image.trim().to_string();
        self.program = self.program.trim().to_string();
        self
    }
}

/// An image reference Chummy will pass to the engine: no spaces or
/// control characters, and it can't look like a flag.
pub fn valid_image(image: &str) -> bool {
    !image.is_empty()
        && image.len() <= 512
        && !image.starts_with('-')
        && image
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-/:@+".contains(c))
}

/// The label on every container Chummy starts.
const LABEL: &str = "com.friendlylabz.chummy";
/// The kept container's workspace (a hash), and its settings (a hash).
const LABEL_WS: &str = "com.friendlylabz.chummy.workspace";
const LABEL_CONFIG: &str = "com.friendlylabz.chummy.config";

/// The home folder of a kept container, in the workspace.
pub const HOME_DIR: &str = ".chummy-home";

pub struct ContainerSandbox {
    opts: ContainerOptions,
    paths: ToolPaths,
    web: reqwest::Client,
    /// Kept containers known to be running, by name.
    kept: tokio::sync::Mutex<BTreeMap<String, PathBuf>>,
}

impl ContainerSandbox {
    pub fn new(opts: ContainerOptions, paths: ToolPaths) -> Result<Self, ToolError> {
        Ok(Self {
            opts: opts.normalized(),
            paths,
            web: web_client()?,
            kept: Default::default(),
        })
    }

    fn program(&self) -> Result<std::path::PathBuf, String> {
        find_program(self.opts.engine.program(), &self.opts.program)
    }

    /// The image calls run: the approved ID.
    fn image_id(&self) -> Result<&str, String> {
        self.opts.approved_id.as_deref().ok_or_else(|| {
            format!(
                "No image approved yet: download “{}” or use a copy on this computer (Security and Privacy → Sandbox).",
                self.opts.image
            )
        })
    }

    /// The `run` arguments of a fresh container for one call (or a check),
    /// up to the image.
    pub fn run_args(
        &self,
        name: &str,
        workspace: Option<&Path>,
        tool: Option<&Path>,
        network: bool,
    ) -> Vec<String> {
        let mut a = self.base_args(name, workspace, network, None);
        if let Some(t) = tool {
            a.push("--mount".into());
            a.push(bind(t, "/tool", true));
            a.push("--mount".into());
            a.push(bind(&self.paths.support, "/chummy", true));
        }
        a
    }

    /// The `run` arguments of the container kept for `workspace`, up to the
    /// image. `labels`: its workspace and settings hashes.
    pub fn keep_args(
        &self,
        name: &str,
        workspace: &Path,
        network: bool,
        labels: (&str, &str),
    ) -> Vec<String> {
        let mut a = self.base_args(name, Some(workspace), network, Some(labels));
        a.push("--mount".into());
        a.push(bind(&self.paths.installed, "/tools", true));
        a.push("--mount".into());
        a.push(bind(&self.paths.support, "/chummy", true));
        a
    }

    fn base_args(
        &self,
        name: &str,
        workspace: Option<&Path>,
        network: bool,
        kept: Option<(&str, &str)>,
    ) -> Vec<String> {
        let o = &self.opts;
        let home = format!("/workspace/{HOME_DIR}");
        let mut a: Vec<String> = match kept {
            // Kept: in the background, with an init that reaps finished
            // processes.
            Some((ws, config)) => vec![
                "run".into(),
                "-d".into(),
                "--init".into(),
                "--rm".into(),
                "--label".into(),
                format!("{LABEL}=workspace"),
                "--label".into(),
                format!("{LABEL_WS}={ws}"),
                "--label".into(),
                format!("{LABEL_CONFIG}={config}"),
                "-e".into(),
                format!("HOME={home}"),
                "-e".into(),
                format!("CHUMMY_HOME={home}"),
            ],
            None => vec![
                "run".into(),
                "--rm".into(),
                "-i".into(),
                "--label".into(),
                format!("{LABEL}=tool"),
                "-e".into(),
                "HOME=/tmp".into(),
            ],
        };
        a.extend(
            [
                "--pull",
                "never",
                "--name",
                name,
                "--read-only",
                "--tmpfs",
                "/tmp:rw,nosuid,nodev,size=256m",
                "--cap-drop",
                "ALL",
                "--security-opt",
                "no-new-privileges",
                "--ipc",
                "none",
                "--pids-limit",
                "256",
                "--log-driver",
                "none",
                "--hostname",
                "chummy",
                "-e",
                "PYTHONDONTWRITEBYTECODE=1",
            ]
            .map(String::from),
        );
        if !network {
            a.push("--network".into());
            a.push("none".into());
        }
        a.push("--memory".into());
        a.push(format!("{}m", o.memory_mb));
        a.push("--memory-swap".into());
        a.push(format!("{}m", o.memory_mb));
        a.push("--cpus".into());
        a.push(format!("{}", o.cpus));
        if let Some(user) = run_as(o.engine, workspace) {
            a.push("--user".into());
            a.push(user);
        }
        if let Some(ws) = workspace {
            a.push("--mount".into());
            a.push(bind(ws, "/workspace", false));
            a.push("--workdir".into());
            a.push("/workspace".into());
        } else {
            a.push("--workdir".into());
            a.push("/tmp".into());
        }
        a
    }

    /// The name of the container kept for `workspace`, and its labels:
    /// the workspace's hash, and the hash of the settings it was started
    /// with (so a change of settings gives a new one). The offline and the
    /// direct-network containers share settings and differ by name.
    fn kept_name(&self, image: &str, workspace: &Path, network: bool) -> (String, String, String) {
        use sha2::Digest;
        let h = |s: &str| crate::lock::hex(&sha2::Sha256::digest(s))[..16].to_string();
        let ws = h(&workspace.display().to_string());
        let mut args = self.keep_args("-", workspace, false, ("-", "-"));
        args.push(image.to_string());
        let config = h(&args.join("\0"));
        let net = if network { "net" } else { "off" };
        (format!("chummy-ws-{ws}-{config}-{net}"), ws, config)
    }

    /// Makes sure the container kept for `workspace` runs, and returns its
    /// name. Replaces the workspace's containers whose settings changed.
    async fn ensure(
        &self,
        program: &Path,
        image: &str,
        workspace: &Path,
        network: bool,
    ) -> Result<String, String> {
        let (name, ws, config) = self.kept_name(image, workspace, network);
        let mut kept = self.kept.lock().await;
        let mut cmd = clean_command(program);
        cmd.args(["inspect", "--format", "{{.State.Running}}", &name]);
        if let Ok((true, out, _)) = process::output(cmd, Duration::from_secs(30)).await {
            if out.trim() == "true" {
                kept.insert(name.clone(), workspace.to_path_buf());
                return Ok(name);
            }
            self.remove(program, std::slice::from_ref(&name)).await;
        }
        kept.remove(&name);
        // This workspace's containers from other settings go.
        let mut cmd = clean_command(program);
        cmd.args([
            "ps",
            "-a",
            "--filter",
            &format!("label={LABEL_WS}={ws}"),
            "--format",
            &format!("{{{{.Names}}}} {{{{.Label \"{LABEL_CONFIG}\"}}}}"),
        ]);
        if let Ok((true, out, _)) = process::output(cmd, Duration::from_secs(30)).await {
            let stale: Vec<String> = out
                .lines()
                .filter_map(|l| l.split_once(' '))
                .filter(|(n, c)| *c != config && !n.is_empty())
                .map(|(n, _)| n.to_string())
                .collect();
            if !stale.is_empty() {
                self.remove(program, &stale).await;
            }
        }
        prepare_home(workspace)?;
        let mut cmd = clean_command(program);
        cmd.args(self.keep_args(&name, workspace, network, (&ws, &config)));
        cmd.args([image, "python3", "-S", "-E", "-c", KEEP_ALIVE]);
        let label = self.opts.engine.label();
        match process::output(cmd, Duration::from_secs(120)).await {
            Ok((true, _, _)) => {}
            Ok((false, _, err)) => {
                return Err(format!(
                    "{label} couldn't start the workspace's container: {}",
                    last_line(&err)
                ));
            }
            Err(e) => {
                return Err(format!(
                    "{label} couldn't start the workspace's container: {e}"
                ));
            }
        }
        kept.insert(name.clone(), workspace.to_path_buf());
        Ok(name)
    }

    async fn remove(&self, program: &Path, names: &[String]) {
        let mut cmd = clean_command(program);
        cmd.args(["rm", "-f"]).args(names);
        let _ = process::output(cmd, Duration::from_secs(60)).await;
    }

    /// Removes the containers this sandbox started for workspaces.
    pub async fn remove_kept(&self) {
        let Ok(program) = self.program() else {
            return;
        };
        let names: Vec<String> = std::mem::take(&mut *self.kept.lock().await)
            .into_keys()
            .collect();
        if !names.is_empty() {
            self.remove(&program, &names).await;
        }
    }

    /// Removes every container Chummy keeps for workspaces, whoever started
    /// it (when Chummy quits, so none are left from a crash either).
    pub async fn remove_all_kept(&self) {
        let Ok(program) = self.program() else {
            return;
        };
        let mut cmd = clean_command(&program);
        cmd.args(["ps", "-aq", "--filter", &format!("label={LABEL}=workspace")]);
        if let Ok((true, out, _)) = process::output(cmd, Duration::from_secs(30)).await {
            let ids: Vec<String> = out.split_whitespace().map(str::to_string).collect();
            if !ids.is_empty() {
                self.remove(&program, &ids).await;
            }
        }
        self.kept.lock().await.clear();
    }

    /// A call in the kept container: `docker exec -i`.
    async fn call_kept(
        &self,
        program: &Path,
        image: &str,
        call: &SandboxCall<'_>,
        workspace: &Path,
        tool: &str,
    ) -> Result<CallOutcome, ToolError> {
        let name = self
            .ensure(program, image, workspace, call.direct_network)
            .await
            .map_err(ToolError::Runtime)?;
        let call_id = call_name();
        let mut cmd = clean_command(program);
        cmd.args(["exec", "-i"]);
        for (k, v) in &call.env {
            cmd.arg("-e").arg(format!("{k}={v}"));
        }
        cmd.arg("-e").arg(format!("CHUMMY_TOOL=/tools/{tool}"));
        cmd.arg("-e").arg(format!("CHUMMY_CALL={call_id}"));
        cmd.args([
            &name,
            "python3",
            "-S",
            "-E",
            "-B",
            "-u",
            "/chummy/_runner.py",
        ]);
        // When the time is up, stop the call's processes (not the
        // container).
        let stop = async {
            let mut k = clean_command(program);
            k.args([
                "exec", &name, "python3", "-S", "-E", "-c", STOP_CALL, &call_id,
            ]);
            let _ = process::output(k, Duration::from_secs(15)).await;
        };
        process::run(
            Run {
                cmd,
                mode: &call.mode,
                web: &self.web,
                policy: &call.network,
                timeout: call.timeout,
                what: self.opts.engine.label(),
            },
            stop,
        )
        .await
    }

    /// Gets the image (`pull`) or, with `pull` false, takes the copy already
    /// on this computer. Returns its ID, to save as `approved_id`.
    pub async fn get_image(&self, pull: bool) -> Result<String, String> {
        let program = self.program()?;
        let image = &self.opts.image;
        if !valid_image(image) {
            return Err(format!("“{image}” isn't an image name Chummy can use."));
        }
        if pull {
            let mut cmd = clean_command(&program);
            cmd.args(["pull", image]);
            let (ok, _, err) = process::output(cmd, Duration::from_secs(30 * 60))
                .await
                .map_err(|e| format!("{} couldn't get the image: {e}", self.opts.engine.label()))?;
            if !ok {
                return Err(format!(
                    "{} couldn't get the image: {}",
                    self.opts.engine.label(),
                    last_line(&err)
                ));
            }
        }
        let mut cmd = clean_command(&program);
        cmd.args(["image", "inspect", "--format", "{{.Id}}", image]);
        let (ok, out, err) = process::output(cmd, Duration::from_secs(30))
            .await
            .map_err(|e| e.to_string())?;
        if !ok {
            return Err(if pull {
                last_line(&err)
            } else {
                format!("“{image}” isn't on this computer yet. Download it instead.")
            });
        }
        let id = out.lines().next().unwrap_or("").trim().to_string();
        if !id.starts_with("sha256:") || id.len() < 20 {
            return Err(format!(
                "{} gave an odd image ID: {id}",
                self.opts.engine.label()
            ));
        }
        Ok(id)
    }

    async fn health(&self) -> Health {
        let kind = SandboxKind::Container;
        let label = self.opts.engine.label();
        let program = match self.program() {
            Ok(p) => p,
            Err(e) => return Health::problem(kind, e),
        };
        let mut cmd = clean_command(&program);
        cmd.args(["version", "--format", "{{.Server.Version}}"]);
        let version = match process::output(cmd, Duration::from_secs(20)).await {
            Ok((true, out, _)) => out,
            Ok((false, _, err)) => {
                return Health::problem(
                    kind,
                    format!("{label} isn't running: {}", last_line(&err)),
                );
            }
            Err(e) => return Health::problem(kind, format!("{label} didn't answer: {e}")),
        };
        let id = match self.image_id() {
            Ok(id) => id.to_string(),
            Err(e) => return Health::problem(kind, e).detail(format!("{label} {version}")),
        };
        let mut cmd = clean_command(&program);
        cmd.args(self.run_args(&call_name(), None, None, false));
        cmd.args([id.as_str(), "python3", "-S", "-E", "-c", PYTHON_PROBE]);
        let health = match process::output(cmd, Duration::from_secs(120)).await {
            Ok((true, out, _)) => match parse_python(&out) {
                Some(py) => Health::ready(
                    kind,
                    py,
                    format!("Tools run in {label}: one container kept for each agent's folder."),
                ),
                None => Health::problem(kind, "The image's Python didn't say its version."),
            },
            Ok((false, _, err)) => Health::problem(
                kind,
                format!(
                    "The image couldn't run Python (it needs python3): {}",
                    last_line(&err)
                ),
            ),
            Err(e) => Health::problem(kind, format!("The image didn't start: {e}")),
        };
        health.detail(format!("{label} {version}")).detail(format!(
            "Image {} ({})",
            self.opts.image,
            short_id(&id)
        ))
    }
}

impl SandboxBackend for ContainerSandbox {
    fn kind(&self) -> SandboxKind {
        SandboxKind::Container
    }

    fn check(&self) -> BoxFut<'_, Health> {
        Box::pin(self.health())
    }

    fn close(&self) -> BoxFut<'_, ()> {
        Box::pin(self.remove_kept())
    }

    fn call<'a>(&'a self, call: SandboxCall<'a>) -> BoxFut<'a, Result<CallOutcome, ToolError>> {
        Box::pin(async move {
            let id = self.image_id().map_err(ToolError::Runtime)?;
            let program = self.program().map_err(ToolError::Runtime)?;
            let ws = call.jail.map(|j| j.root().to_path_buf());
            // With a workspace: its kept container, when the tool is one of
            // the installed ones (mounted at /tools).
            let tool = call
                .tool_dir
                .strip_prefix(&self.paths.installed)
                .ok()
                .and_then(|rest| rest.to_str())
                .filter(|t| !t.is_empty() && !t.contains(['/', '\\']))
                .map(str::to_string);
            if let (Some(ws), Some(tool)) = (ws.as_deref(), tool) {
                return self.call_kept(&program, id, &call, ws, &tool).await;
            }
            let name = call_name();
            let mut cmd = clean_command(&program);
            cmd.args(self.run_args(
                &name,
                ws.as_deref(),
                Some(call.tool_dir),
                call.direct_network,
            ));
            for (k, v) in &call.env {
                cmd.arg("-e").arg(format!("{k}={v}"));
            }
            cmd.args([id, "python3", "-S", "-E", "-B", "-u", "/chummy/_runner.py"]);
            let kill = async {
                let mut k = clean_command(&program);
                k.args(["kill", &name]);
                let _ = process::output(k, Duration::from_secs(15)).await;
            };
            process::run(
                Run {
                    cmd,
                    mode: &call.mode,
                    web: &self.web,
                    policy: &call.network,
                    timeout: call.timeout,
                    what: self.opts.engine.label(),
                },
                kill,
            )
            .await
        })
    }
}

/// What a kept container runs between calls.
const KEEP_ALIVE: &str = "import time\nwhile True: time.sleep(86400)";

/// Stops a call in a kept container: the process group of the runner whose
/// `CHUMMY_CALL` is `sys.argv[1]` (never the container's own).
const STOP_CALL: &str = r#"import os, sys, signal
want = ("CHUMMY_CALL=" + sys.argv[1]).encode()
init = os.getpgid(1)
groups = set()
for p in os.listdir("/proc"):
    if not p.isdigit():
        continue
    try:
        with open(f"/proc/{p}/environ", "rb") as f:
            if want in f.read().split(b"\0"):
                groups.add(os.getpgid(int(p)))
    except OSError:
        pass
for g in groups - {init, os.getpgrp()}:
    try:
        os.killpg(g, signal.SIGKILL)
    except OSError:
        pass
"#;

/// Makes the kept container's home folder in the workspace, ignored by git.
fn prepare_home(workspace: &Path) -> Result<(), String> {
    let home = workspace.join(HOME_DIR);
    let meta = std::fs::symlink_metadata(&home);
    if let Ok(m) = &meta
        && !m.is_dir()
    {
        return Err(format!(
            "“{HOME_DIR}” in the workspace isn't a folder: the sandbox keeps its home there. Move it away."
        ));
    }
    std::fs::create_dir_all(&home)
        .map_err(|e| format!("couldn't make “{HOME_DIR}” in the workspace: {e}"))?;
    let ignore = home.join(".gitignore");
    if !ignore.exists() {
        let _ = std::fs::write(
            &ignore,
            "# Chummy's sandbox keeps its home here (git settings, caches,\n# programs installed for the user). Ignored by git.\n*\n",
        );
    }
    Ok(())
}

/// A unique container name for one call.
fn call_name() -> String {
    use sha2::Digest;
    let seed = format!(
        "{}-{}-{:?}",
        std::process::id(),
        super::now_ms(),
        std::thread::current().id()
    );
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let h = sha2::Sha256::digest(format!("{seed}-{n}"));
    format!("chummy-tool-{}", &crate::lock::hex(&h)[..16])
}

pub fn short_id(id: &str) -> String {
    let hex = id.strip_prefix("sha256:").unwrap_or(id);
    format!("sha256:{}", &hex[..hex.len().min(12)])
}

/// A `--mount` value. Docker reads it as CSV, so a path with a comma or a
/// quote is quoted.
fn bind(src: &Path, target: &str, readonly: bool) -> String {
    let shown = src.display().to_string();
    // Windows' canonical paths start with \\?\, which engines don't take.
    let shown = shown
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| shown.strip_prefix(r"\\?\").map(str::to_string))
        .unwrap_or(shown);
    let source = format!("source={shown}");
    let source = if source.contains([',', '"']) {
        format!("\"{}\"", source.replace('"', "\"\""))
    } else {
        source
    };
    format!(
        "type=bind,{source},target={target}{}",
        if readonly { ",readonly" } else { "" }
    )
}

/// Who the tool runs as. Docker: the workspace owner's user and group on
/// Linux and macOS, so files it writes belong to you and it isn't root.
/// Rootless Podman already maps the container's root to you.
fn run_as(engine: ContainerEngine, workspace: Option<&Path>) -> Option<String> {
    if engine == ContainerEngine::Podman {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = workspace.and_then(|w| std::fs::metadata(w).ok());
        Some(match m {
            Some(m) => format!("{}:{}", m.uid(), m.gid()),
            None => "65534:65534".into(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = workspace;
        Some("65534:65534".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox() -> ContainerSandbox {
        ContainerSandbox::new(
            ContainerOptions {
                approved_id: Some("sha256:0123456789abcdef0123456789abcdef".into()),
                ..Default::default()
            },
            ToolPaths::new("/chummy/tools"),
        )
        .unwrap()
    }

    #[test]
    fn calls_are_locked_down() {
        let a = sandbox().run_args("n", Some(Path::new("/ws")), Some(Path::new("/t")), false);
        let joined = a.join(" ");
        for must in [
            "--network none",
            "--read-only",
            "--cap-drop ALL",
            "--security-opt no-new-privileges",
            "--pull never",
            "--ipc none",
            "--log-driver none",
            "--memory 1024m",
            "--memory-swap 1024m",
            "--pids-limit 256",
            "type=bind,source=/ws,target=/workspace",
            "type=bind,source=/t,target=/tool,readonly",
            "target=/chummy,readonly",
            "--workdir /workspace",
        ] {
            assert!(joined.contains(must), "missing {must}: {joined}");
        }
        assert!(!joined.contains("docker.sock"));
        assert!(!joined.contains("--privileged"));
        // Exactly three mounts.
        assert_eq!(a.iter().filter(|x| *x == "--mount").count(), 3);
    }

    #[test]
    fn kept_containers_mount_the_workspace_tools_and_runner_only() {
        let b = sandbox();
        let a = b.keep_args("k", Path::new("/ws"), false, ("w", "c"));
        let joined = a.join(" ");
        for must in [
            "run -d --init --rm",
            "--label com.friendlylabz.chummy=workspace",
            "--label com.friendlylabz.chummy.workspace=w",
            "--label com.friendlylabz.chummy.config=c",
            "-e HOME=/workspace/.chummy-home",
            "--read-only",
            "--cap-drop ALL",
            "--security-opt no-new-privileges",
            "--network none",
            "type=bind,source=/ws,target=/workspace",
            "--workdir /workspace",
        ] {
            assert!(joined.contains(must), "missing {must}: {joined}");
        }
        let mounts: Vec<_> = a
            .iter()
            .zip(a.iter().skip(1))
            .filter(|(k, _)| *k == "--mount")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(mounts.len(), 3, "{mounts:?}");
        assert!(mounts[1].ends_with("target=/tools,readonly"), "{mounts:?}");
        assert!(mounts[2].ends_with("target=/chummy,readonly"), "{mounts:?}");
        assert!(
            !joined.contains(" -i "),
            "kept containers aren't attached: {joined}"
        );
    }

    #[test]
    fn kept_containers_change_with_their_settings() {
        let b = sandbox();
        let img = "sha256:0123456789abcdef0123456789abcdef";
        let (name, ws, config) = b.kept_name(img, Path::new("/ws"), false);
        assert!(name.starts_with("chummy-ws-") && name.contains(&ws) && name.contains(&config));
        // Same settings, same container.
        assert_eq!(b.kept_name(img, Path::new("/ws"), false).0, name);
        // Another workspace, the network, the image or the limits: another.
        let (other_ws, ws2, _) = b.kept_name(img, Path::new("/ws2"), false);
        assert!(other_ws != name && ws2 != ws);
        // The direct-network one is another container with the same settings.
        let (online, ws3, c3) = b.kept_name(img, Path::new("/ws"), true);
        assert!(online != name && ws3 == ws && c3 == config);
        assert_ne!(b.kept_name("sha256:other", Path::new("/ws"), false).0, name);
        let mut opts = b.opts.clone();
        opts.memory_mb = 4096;
        let bigger = ContainerSandbox::new(opts, ToolPaths::new("/chummy/tools")).unwrap();
        let (n4, ws4, c4) = bigger.kept_name(img, Path::new("/ws"), false);
        assert!(n4 != name && ws4 == ws && c4 != config);
    }

    #[test]
    fn the_home_folder_ignores_itself() {
        let ws = tempfile::tempdir().unwrap();
        prepare_home(ws.path()).unwrap();
        let ignore = std::fs::read_to_string(ws.path().join(".chummy-home/.gitignore")).unwrap();
        assert!(ignore.lines().any(|l| l == "*"), "{ignore}");
        // Kept as it is the next time.
        std::fs::write(ws.path().join(".chummy-home/.gitignore"), "mine").unwrap();
        prepare_home(ws.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(ws.path().join(".chummy-home/.gitignore")).unwrap(),
            "mine"
        );
        // A file in the way is explained.
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join(".chummy-home"), "x").unwrap();
        assert!(
            prepare_home(other.path())
                .unwrap_err()
                .contains("isn't a folder")
        );
    }

    #[test]
    fn only_a_direct_network_call_gets_a_network() {
        let on = sandbox().run_args("n", Some(Path::new("/ws")), Some(Path::new("/t")), true);
        assert!(!on.iter().any(|x| x == "--network"), "{on:?}");
        // Everything else stays locked down.
        let joined = on.join(" ");
        for must in [
            "--read-only",
            "--cap-drop ALL",
            "--security-opt no-new-privileges",
        ] {
            assert!(joined.contains(must), "missing {must}: {joined}");
        }
    }

    #[test]
    fn network_words() {
        use crate::net::NetPolicy;
        use crate::sandbox::{SandboxKind, network_word};
        let on = NetPolicy::on();
        let off = NetPolicy::off("no");
        assert_eq!(network_word(SandboxKind::Container, &on, true), "direct");
        assert_eq!(network_word(SandboxKind::Openshell, &on, true), "web");
        assert_eq!(network_word(SandboxKind::Container, &on, false), "web");
        assert_eq!(network_word(SandboxKind::Container, &off, true), "off");
    }

    #[test]
    fn checks_mount_nothing() {
        let a = sandbox().run_args("n", None, None, false);
        assert!(!a.iter().any(|x| x == "--mount"));
    }

    #[test]
    fn odd_paths_are_quoted_for_mounts() {
        assert_eq!(
            bind(Path::new("/a,b"), "/workspace", false),
            "type=bind,\"source=/a,b\",target=/workspace"
        );

        // (was windows_long_paths_are_shortened)
        {
            assert_eq!(
                bind(Path::new(r"\\?\C:\Users\me\ws"), "/workspace", false),
                r"type=bind,source=C:\Users\me\ws,target=/workspace"
            );
        }
    }

    #[test]
    fn image_names_are_checked() {
        assert!(valid_image("python:3.14-slim"));
        assert!(valid_image("ghcr.io/me/img@sha256:abc"));
        assert!(!valid_image("--privileged"));
        assert!(!valid_image("img name"));
        assert!(!valid_image(""));

        // (was names_are_unique)
        {
            assert_ne!(call_name(), call_name());
        }

        // (was limits_are_clamped)
        {
            let o = ContainerOptions {
                memory_mb: 1,
                cpus: f64::NAN,
                ..Default::default()
            }
            .normalized();
            assert_eq!((o.memory_mb, o.cpus), (128, 2.0));
        }
    }

    #[test]
    fn calls_without_an_image_explain() {
        let s = ContainerSandbox::new(ContainerOptions::default(), ToolPaths::new("/x")).unwrap();
        assert!(s.image_id().unwrap_err().contains("Security and Privacy"));
    }
}
