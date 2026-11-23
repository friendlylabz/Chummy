//! Sandboxes: where tool calls run.
//!
//! Chummy's loop, its model calls and every permission check always run in
//! Chummy itself. What a sandbox decides is where a *tool call* runs: the
//! one piece of user-defined code a loop can start. Every backend gets the
//! same three things and nothing else:
//!
//! - the agent's workspace (and only it) at `/workspace`, read/write;
//! - the tool and Chummy's runner, read-only;
//! - no network of its own. `chummy.http` asks Chummy over the tool's
//!   stdin/stdout, so the app's [`NetPolicy`] decides every request, the
//!   same way in every backend.
//!
//! The backends:
//!
//! | Kind                      | What runs the tool                          | Programs |
//! |---------------------------|---------------------------------------------|----------|
//! | [`SandboxKind::Native`]   | CPython for WebAssembly under Wasmtime (built in) | no |
//! | [`SandboxKind::Container`]| a hardened Docker or Podman container       | yes      |
//! | [`SandboxKind::Openshell`]| an NVIDIA OpenShell sandbox (copies the workspace in and out) | yes |
//!
//! Tools keep one format everywhere (a PEP 723 Python file). A tool says
//! what it needs beyond Python with `needs = [...]` and `requires-python`;
//! [`unusable`] explains when the chosen sandbox can't run it.
//!
//! Adding a backend: implement [`SandboxBackend`], add a [`SandboxKind`]
//! variant and its options to [`SandboxSettings`], and build it in the app.

pub mod container;
pub mod native;
pub mod openshell;
mod process;
pub mod sync;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use chummy_sandbox::Jail;
use serde::{Deserialize, Serialize};

use crate::host::{CallOutcome, Mode};
use crate::install::{ToolManifest, manifest, verify};
use crate::meta::python_satisfies;
use crate::net::NetPolicy;
use crate::python::ToolPaths;
use crate::{ToolError, ToolResult};

pub use container::{ContainerEngine, ContainerOptions, ContainerSandbox};
pub use native::NativeSandbox;
pub use openshell::{OpenShellOptions, OpenShellSandbox};

/// A boxed future, so [`SandboxBackend`] can be used as `dyn`.
pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Which sandbox runs tools. One for the whole app.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxKind {
    /// WebAssembly, built into Chummy. Nothing to install.
    #[default]
    Native,
    /// A Docker or Podman container.
    Container,
    /// An NVIDIA OpenShell sandbox.
    Openshell,
}

impl SandboxKind {
    pub const ALL: [SandboxKind; 3] = [Self::Native, Self::Container, Self::Openshell];

    pub fn label(self) -> &'static str {
        match self {
            Self::Native => "Native (WebAssembly)",
            Self::Container => "Container (Docker or Podman)",
            Self::Openshell => "NVIDIA OpenShell",
        }
    }

    /// Whether tools in this sandbox can start other programs.
    pub fn programs(self) -> bool {
        !matches!(self, Self::Native)
    }
}

/// The sandbox settings, as kept in the app's settings (and, later,
/// chummyd's master config).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxSettings {
    pub kind: SandboxKind,
    pub container: ContainerOptions,
    pub openshell: OpenShellOptions,
}

/// Whether a sandbox can run tools right now, and what it offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub kind: SandboxKind,
    pub ready: bool,
    /// One plain sentence: what's working, or what to do.
    pub message: String,
    /// The Python tools run on (like "3.14.7"), when known.
    pub python: Option<String>,
    /// Tools can start other programs.
    pub programs: bool,
    /// Extra facts for the settings page ("Docker 28.1", the image id…).
    pub details: Vec<String>,
    /// When this was checked (ms since 1970).
    pub checked_at: u64,
}

impl Health {
    pub fn ready(kind: SandboxKind, python: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind,
            ready: true,
            message: message.into(),
            python: Some(python.into()),
            programs: kind.programs(),
            details: vec![],
            checked_at: now_ms(),
        }
    }

    pub fn problem(kind: SandboxKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            ready: false,
            message: message.into(),
            python: None,
            programs: kind.programs(),
            details: vec![],
            checked_at: now_ms(),
        }
    }

    /// Not checked yet.
    pub fn unknown(kind: SandboxKind) -> Self {
        Self {
            checked_at: 0,
            ..Self::problem(kind, "Not checked yet.")
        }
    }

    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.details.push(d.into());
        self
    }
}

/// One tool call, as a backend sees it.
pub struct SandboxCall<'a> {
    /// The installed tool's folder (tool.py and site-packages).
    pub tool_dir: &'a Path,
    pub mode: Mode,
    /// The agent's workspace. `None`: no workspace (nothing is mounted).
    pub jail: Option<&'a Jail>,
    pub network: NetPolicy,
    pub timeout: Duration,
    /// Extra environment variables (e.g. `TZ`). The host's own environment
    /// is never passed through.
    pub env: Vec<(String, String)>,
}

/// A place tool calls can run.
pub trait SandboxBackend: Send + Sync {
    fn kind(&self) -> SandboxKind;

    /// Whether it's set up and working, and which Python it has. May be
    /// slow (it can start a container); the app caches the answer.
    fn check(&self) -> BoxFut<'_, Health>;

    /// Runs one call. Problems the tool reports come back in the outcome;
    /// only sandbox failures are `Err`.
    fn call<'a>(&'a self, call: SandboxCall<'a>) -> BoxFut<'a, Result<CallOutcome, ToolError>>;
}

/// Why a tool can't run in a sandbox with this health, or `None` if it can.
/// Words for the settings page, so the tool can be shown greyed out.
pub fn unusable(m: &ToolManifest, health: &Health) -> Option<String> {
    if m.needs.iter().any(|n| n == "programs") && !health.programs {
        return Some(
            "Needs a sandbox that can run programs. Choose Container or OpenShell in Security and Privacy."
                .into(),
        );
    }
    if !health.ready {
        return Some(format!("The sandbox isn't ready: {}", health.message));
    }
    if let (Some(req), Some(py)) = (&m.requires_python, &health.python)
        && !python_satisfies(req, py).unwrap_or(true)
    {
        return Some(format!(
            "Needs Python {req}, and this sandbox has Python {py}. Use an image with a newer Python."
        ));
    }
    None
}

/// Calls an installed tool in `backend`. Problems the tool reports (a
/// missing file, bad arguments) come back as an error result for the model;
/// only sandbox failures are `Err`.
pub async fn run_tool(
    backend: &dyn SandboxBackend,
    paths: &ToolPaths,
    name: &str,
    args: serde_json::Value,
    jail: Option<&Jail>,
    network: NetPolicy,
    env: Vec<(String, String)>,
) -> Result<ToolResult, ToolError> {
    let m = manifest(paths, name).ok_or_else(|| ToolError::NotInstalled(name.into()))?;
    verify(paths, &m)?;
    let dir = paths.tool_dir(name);
    let out = backend
        .call(SandboxCall {
            tool_dir: &dir,
            mode: Mode::Call(args),
            jail,
            network,
            timeout: Duration::from_secs(m.timeout_secs as u64),
            env,
        })
        .await?;
    Ok(crate::to_result(out))
}

/// Finds a program: `custom` if set (it must exist), otherwise on `PATH`
/// and in the usual install places (apps started from a desktop often have
/// a short `PATH`, especially on macOS).
pub fn find_program(name: &str, custom: &str) -> Result<PathBuf, String> {
    let custom = custom.trim();
    if !custom.is_empty() {
        let p = PathBuf::from(custom);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!("There's no program at {custom}."))
        };
    }
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        dirs.extend(
            [
                "/usr/local/bin",
                "/opt/homebrew/bin",
                "/Applications/Docker.app/Contents/Resources/bin",
                "/opt/podman/bin",
            ]
            .map(PathBuf::from),
        );
    } else if cfg!(windows) {
        dirs.extend(
            [
                r"C:\Program Files\Docker\Docker\resources\bin",
                r"C:\Program Files\RedHat\Podman",
            ]
            .map(PathBuf::from),
        );
    } else {
        dirs.extend(["/usr/bin", "/usr/local/bin", "/snap/bin"].map(PathBuf::from));
    }
    if let Some(h) = home {
        for d in [".docker/bin", ".rd/bin", ".local/bin", ".orbstack/bin"] {
            dirs.push(h.join(d));
        }
    }
    dirs.into_iter()
        .map(|d| d.join(&exe))
        .find(|p| p.is_file())
        .ok_or_else(|| format!("Chummy couldn't find {name}. Install it, or set where it is."))
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The last non-empty line of a program's error output, for messages.
pub(crate) fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(400)
        .collect()
}

/// The Python version a probe printed ("3.14.7"), if it looks like one.
pub(crate) fn parse_python(out: &str) -> Option<String> {
    let v = out.lines().map(str::trim).find(|l| !l.is_empty())?;
    (v.split('.').count() == 3 && v.split('.').all(|p| p.parse::<u32>().is_ok()))
        .then(|| v.to_string())
}

/// Prints the Python version. Run with `python3 -S -E -c`.
pub(crate) const PYTHON_PROBE: &str = "import sys; print('%d.%d.%d' % sys.version_info[:3])";

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(needs: &[&str], py: Option<&str>) -> ToolManifest {
        ToolManifest {
            name: "t".into(),
            title: "T".into(),
            version: "1".into(),
            description: "d".into(),
            parameters: serde_json::json!({"type": "object"}),
            network: false,
            risky: false,
            timeout_secs: 5,
            needs: needs.iter().map(|s| s.to_string()).collect(),
            requires_python: py.map(str::to_string),
            dependencies: vec![],
            packages: vec![],
            python: "3.14.7".into(),
            source_sha256: String::new(),
            builtin: false,
            installed_at: 0,
        }
    }

    #[test]
    fn tools_that_need_programs_are_unusable_in_webassembly() {
        let native = Health::ready(SandboxKind::Native, "3.14.7", "ok");
        let container = Health::ready(SandboxKind::Container, "3.14.2", "ok");
        assert!(unusable(&tool(&[], Some(">=3.14")), &native).is_none());
        let why = unusable(&tool(&["programs"], None), &native).unwrap();
        assert!(why.contains("run programs"), "{why}");
        assert!(unusable(&tool(&["programs"], None), &container).is_none());
    }

    #[test]
    fn tools_need_a_new_enough_python() {
        let old = Health::ready(SandboxKind::Container, "3.12.4", "ok");
        let why = unusable(&tool(&[], Some(">=3.14")), &old).unwrap();
        assert!(why.contains("3.12.4") && why.contains(">=3.14"), "{why}");
        assert!(unusable(&tool(&[], None), &old).is_none());
    }

    #[test]
    fn nothing_runs_in_a_sandbox_that_isnt_ready() {
        let down = Health::problem(SandboxKind::Container, "Docker isn't running.");
        let why = unusable(&tool(&[], None), &down).unwrap();
        assert!(why.contains("Docker isn't running"), "{why}");
    }

    #[test]
    fn python_versions_are_parsed() {
        assert_eq!(parse_python("3.14.7\n").as_deref(), Some("3.14.7"));
        assert_eq!(parse_python("\n 3.12.10 \n").as_deref(), Some("3.12.10"));
        assert_eq!(parse_python("Python 3.12"), None);
        assert_eq!(parse_python(""), None);
    }

    #[test]
    fn settings_default_to_native() {
        let s: SandboxSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.kind, SandboxKind::Native);
        let s: SandboxSettings = serde_json::from_str(r#"{"kind":"openshell"}"#).unwrap();
        assert_eq!(s.kind, SandboxKind::Openshell);
    }
}
