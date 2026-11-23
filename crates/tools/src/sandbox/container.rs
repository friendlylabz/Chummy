//! The container sandbox: each tool call runs in a fresh, hardened Docker or
//! Podman container.
//!
//! Chummy never installs Docker or Podman and never downloads an image on
//! its own: the user picks an image and gets it in Security and Privacy,
//! which records the image's ID. Calls run that exact ID (`--pull never`),
//! so a tag that changes later can't swap in a different image unseen.
//!
//! Each call is `docker run --rm -i` with:
//!
//! - `--network none`: no network at all. `chummy.http` still works, over
//!   stdin/stdout, through Chummy's own checks.
//! - only three mounts: the workspace (read/write) at `/workspace`, the tool
//!   at `/tool` and Chummy's runner at `/chummy` (both read-only). The
//!   Docker socket is never mounted.
//! - `--read-only` root filesystem with a small `/tmp`, `--cap-drop ALL`,
//!   `--security-opt no-new-privileges`, `--ipc none`, limits on memory,
//!   CPUs and processes, no logs kept by the engine, and (with Docker on
//!   Linux and macOS) the user id of the workspace's owner rather than root.
//! - no host environment: only the variables Chummy passes (`TZ`).

use std::path::Path;
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

pub struct ContainerSandbox {
    opts: ContainerOptions,
    paths: ToolPaths,
    web: reqwest::Client,
}

impl ContainerSandbox {
    pub fn new(opts: ContainerOptions, paths: ToolPaths) -> Result<Self, ToolError> {
        Ok(Self {
            opts: opts.normalized(),
            paths,
            web: web_client()?,
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

    /// The `run` arguments shared by calls and checks, up to the image.
    pub fn run_args(
        &self,
        name: &str,
        workspace: Option<&Path>,
        tool: Option<&Path>,
    ) -> Vec<String> {
        let o = &self.opts;
        let mut a: Vec<String> = [
            "run",
            "--rm",
            "-i",
            "--pull",
            "never",
            "--name",
            name,
            "--label",
            "com.friendlylabz.chummy=tool",
            "--network",
            "none",
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
            "HOME=/tmp",
            "-e",
            "PYTHONDONTWRITEBYTECODE=1",
        ]
        .map(String::from)
        .to_vec();
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
        if let Some(t) = tool {
            a.push("--mount".into());
            a.push(bind(t, "/tool", true));
            a.push("--mount".into());
            a.push(bind(&self.paths.support, "/chummy", true));
        }
        a
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
        cmd.args(self.run_args(&call_name(), None, None));
        cmd.args([id.as_str(), "python3", "-S", "-E", "-c", PYTHON_PROBE]);
        let health = match process::output(cmd, Duration::from_secs(120)).await {
            Ok((true, out, _)) => match parse_python(&out) {
                Some(py) => Health::ready(
                    kind,
                    py,
                    format!("Tools run in {label} containers with no network."),
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

    fn call<'a>(&'a self, call: SandboxCall<'a>) -> BoxFut<'a, Result<CallOutcome, ToolError>> {
        Box::pin(async move {
            let id = self.image_id().map_err(ToolError::Runtime)?;
            let program = self.program().map_err(ToolError::Runtime)?;
            let name = call_name();
            let ws = call.jail.map(|j| j.root().to_path_buf());
            let mut cmd = clean_command(&program);
            cmd.args(self.run_args(&name, ws.as_deref(), Some(call.tool_dir)));
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
        let a = sandbox().run_args("n", Some(Path::new("/ws")), Some(Path::new("/t")));
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
    fn checks_mount_nothing() {
        let a = sandbox().run_args("n", None, None);
        assert!(!a.iter().any(|x| x == "--mount"));
    }

    #[test]
    fn odd_paths_are_quoted_for_mounts() {
        assert_eq!(
            bind(Path::new("/a,b"), "/workspace", false),
            "type=bind,\"source=/a,b\",target=/workspace"
        );
    }

    #[test]
    fn windows_long_paths_are_shortened() {
        assert_eq!(
            bind(Path::new(r"\\?\C:\Users\me\ws"), "/workspace", false),
            r"type=bind,source=C:\Users\me\ws,target=/workspace"
        );
    }

    #[test]
    fn image_names_are_checked() {
        assert!(valid_image("python:3.14-slim"));
        assert!(valid_image("ghcr.io/me/img@sha256:abc"));
        assert!(!valid_image("--privileged"));
        assert!(!valid_image("img name"));
        assert!(!valid_image(""));
    }

    #[test]
    fn calls_without_an_image_explain() {
        let s = ContainerSandbox::new(ContainerOptions::default(), ToolPaths::new("/x")).unwrap();
        assert!(s.image_id().unwrap_err().contains("Security and Privacy"));
    }

    #[test]
    fn limits_are_clamped() {
        let o = ContainerOptions {
            memory_mb: 1,
            cpus: f64::NAN,
            ..Default::default()
        }
        .normalized();
        assert_eq!((o.memory_mb, o.cpus), (128, 2.0));
    }

    #[test]
    fn names_are_unique() {
        assert_ne!(call_name(), call_name());
    }
}
