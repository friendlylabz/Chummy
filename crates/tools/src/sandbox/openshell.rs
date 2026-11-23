//! The OpenShell sandbox: tool calls run in NVIDIA OpenShell sandboxes,
//! through the `openshell` command-line program (which talks to the user's
//! OpenShell gateway). Chummy never installs OpenShell or sets up a gateway.
//!
//! - **Policy.** Chummy creates its sandboxes with its own policy
//!   ([`POLICY`]): Landlock is required, the process runs as the
//!   unprivileged `sandbox` user, only system folders are readable and only
//!   `/sandbox` and `/tmp` writable, and there are no network policies at
//!   all, so nothing in the sandbox can connect anywhere. `chummy.http`
//!   still works, over the call's stdin/stdout, through Chummy's checks.
//! - **One sandbox per workspace**, kept between calls (creating one takes
//!   a while), named from a hash of the workspace's location, so two agents
//!   never share one. "Remove Chummy's sandboxes" deletes them.
//! - **The workspace is copied in and out** for each call, because
//!   OpenShell can't mount a folder: see [`super::sync`] for how changes come
//!   back safely. Each call works in its own `/sandbox/<call id>` folder,
//!   removed afterwards.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::process::{self, Run, START_GRACE, clean_command};
use super::sync::{self, LIMITS};
use super::{
    BoxFut, Health, PYTHON_PROBE, SandboxBackend, SandboxCall, SandboxKind, find_program,
    last_line, parse_python,
};
use crate::ToolError;
use crate::host::{CallOutcome, web_client};
use crate::lock::hex;
use crate::python::ToolPaths;

/// OpenShell settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenShellOptions {
    /// Where `openshell` is. Empty: find it.
    pub program: String,
    /// The sandbox image (`--from`). Empty: OpenShell's default.
    pub image: String,
}

impl OpenShellOptions {
    pub fn normalized(mut self) -> Self {
        self.program = self.program.trim().to_string();
        self.image = self.image.trim().to_string();
        self
    }
}

/// The policy Chummy's sandboxes are created with. Changing it changes
/// [`POLICY_VERSION`], so new sandboxes are made with the new one.
pub const POLICY: &str = "\
# Chummy's OpenShell policy: deny everything a tool doesn't need.
version: 1
filesystem_policy:
  include_workdir: false
  read_only:
    - /usr
    - /lib
    - /etc
  read_write:
    - /sandbox
    - /tmp
landlock:
  compatibility: hard_requirement
process:
  run_as_user: sandbox
  run_as_group: sandbox
# No network_policies: nothing in the sandbox may connect anywhere.
";

pub const POLICY_VERSION: &str = "p1";

/// The label on every sandbox Chummy makes.
const LABEL: &str = "chummy=tools";

pub struct OpenShellSandbox {
    opts: OpenShellOptions,
    paths: ToolPaths,
    web: reqwest::Client,
    /// Where calls are staged (under the Chummy folder).
    scratch: PathBuf,
    /// Sandboxes known to exist.
    made: tokio::sync::Mutex<BTreeSet<String>>,
}

impl OpenShellSandbox {
    /// `scratch` is a folder Chummy owns, for copies of workspaces.
    pub fn new(
        opts: OpenShellOptions,
        paths: ToolPaths,
        scratch: PathBuf,
    ) -> Result<Self, ToolError> {
        Ok(Self {
            opts: opts.normalized(),
            paths,
            web: web_client()?,
            scratch,
            made: Default::default(),
        })
    }

    fn program(&self) -> Result<PathBuf, String> {
        find_program("openshell", &self.opts.program)
    }

    async fn cli(&self, args: &[&str], limit: Duration) -> Result<(bool, String, String), String> {
        let program = self.program()?;
        let mut cmd = clean_command(&program);
        cmd.args(args);
        process::output(cmd, limit)
            .await
            .map_err(|e| format!("OpenShell didn't answer: {e}"))
    }

    /// Makes sure the sandbox `name` exists, creating it if needed.
    async fn ensure(&self, name: &str) -> Result<(), String> {
        let mut made = self.made.lock().await;
        if made.contains(name) {
            return Ok(());
        }
        let (exists, _, _) = self
            .cli(&["sandbox", "get", name], Duration::from_secs(60))
            .await?;
        if !exists {
            std::fs::create_dir_all(&self.scratch)
                .map_err(|e| format!("couldn't prepare the sandbox: {e}"))?;
            let policy = self.scratch.join(format!("policy-{POLICY_VERSION}.yaml"));
            std::fs::write(&policy, POLICY)
                .map_err(|e| format!("couldn't write the policy: {e}"))?;
            let policy = policy.display().to_string();
            let mut args = vec![
                "sandbox",
                "create",
                "--name",
                name,
                "--policy",
                &policy,
                "--detach",
                "--no-tty",
                "--no-auto-providers",
                "--label",
                LABEL,
            ];
            if !self.opts.image.is_empty() {
                args.extend(["--from", self.opts.image.as_str()]);
            }
            let (ok, out, err) = self.cli(&args, Duration::from_secs(15 * 60)).await?;
            if !ok {
                let why = last_line(if err.is_empty() { &out } else { &err });
                return Err(format!("OpenShell couldn't create a sandbox: {why}"));
            }
        }
        made.insert(name.to_string());
        Ok(())
    }

    /// Deletes every sandbox Chummy made. Returns how many.
    pub async fn remove_all(&self) -> Result<usize, String> {
        let (ok, out, err) = self
            .cli(
                &["sandbox", "list", "--selector", LABEL, "--names"],
                Duration::from_secs(60),
            )
            .await?;
        if !ok {
            return Err(format!(
                "OpenShell couldn't list sandboxes: {}",
                last_line(&err)
            ));
        }
        let names: Vec<&str> = out
            .lines()
            .map(str::trim)
            .filter(|n| n.starts_with("chummy-"))
            .collect();
        if !names.is_empty() {
            let mut args = vec!["sandbox", "delete"];
            args.extend(names.iter().copied());
            let (ok, _, err) = self.cli(&args, Duration::from_secs(300)).await?;
            if !ok {
                return Err(format!(
                    "OpenShell couldn't delete them: {}",
                    last_line(&err)
                ));
            }
        }
        self.made.lock().await.clear();
        Ok(names.len())
    }

    async fn health(&self) -> Health {
        let kind = SandboxKind::Openshell;
        if let Err(e) = self.program() {
            return Health::problem(kind, e);
        }
        match self.cli(&["status"], Duration::from_secs(30)).await {
            Ok((true, _, _)) => {}
            Ok((false, out, err)) => {
                let why = last_line(if err.is_empty() { &out } else { &err });
                return Health::problem(
                    kind,
                    format!("OpenShell's gateway isn't reachable: {why}"),
                );
            }
            Err(e) => return Health::problem(kind, e),
        }
        let name = format!("chummy-check-{POLICY_VERSION}");
        if let Err(e) = self.ensure(&name).await {
            return Health::problem(kind, e);
        }
        let probe = self
            .cli(
                &[
                    "sandbox",
                    "exec",
                    "-n",
                    &name,
                    "--no-tty",
                    "--no-login-shell",
                    "--timeout",
                    "60",
                    "--",
                    "python3",
                    "-S",
                    "-E",
                    "-c",
                    PYTHON_PROBE,
                ],
                Duration::from_secs(90),
            )
            .await;
        let image = if self.opts.image.is_empty() {
            "OpenShell's default image".to_string()
        } else {
            self.opts.image.clone()
        };
        let health = match probe {
            Ok((true, out, _)) => match parse_python(&out) {
                Some(py) => Health::ready(
                    kind,
                    py,
                    "Tools run in OpenShell sandboxes with no network; the workspace is copied in and out for each call.",
                ),
                None => Health::problem(kind, "The sandbox's Python didn't say its version."),
            },
            Ok((false, _, err)) => Health::problem(
                kind,
                format!(
                    "The sandbox couldn't run Python (its image needs python3): {}",
                    last_line(&err)
                ),
            ),
            Err(e) => Health::problem(kind, e),
        };
        health
            .detail("Gateway connected")
            .detail(format!("Image: {image}"))
            .detail(format!(
                "Policy {POLICY_VERSION}: no network, Landlock required"
            ))
    }

    async fn run_call(&self, call: SandboxCall<'_>) -> Result<CallOutcome, ToolError> {
        let rt = ToolError::Runtime;
        let program = self.program().map_err(rt)?;
        let name = sandbox_name(call.jail.map(|j| j.root()));
        self.ensure(&name).await.map_err(rt)?;

        // Stage: workspace, tool and runner, under one call folder.
        let id = call_id();
        let local = self.scratch.join(&id);
        let _cleanup = RemoveOnDrop(local.clone());
        let snapshot = match call.jail {
            Some(j) => Some(sync::stage(j, &local.join("workspace"), LIMITS).map_err(rt)?),
            None => {
                std::fs::create_dir_all(local.join("workspace")).map_err(|e| rt(e.to_string()))?;
                None
            }
        };
        copy_tree(call.tool_dir, &local.join("tool")).map_err(rt)?;
        copy_tree(&self.paths.support, &local.join("chummy")).map_err(rt)?;
        let local_s = local.display().to_string();
        let upload = [
            "sandbox",
            "upload",
            &name,
            &local_s,
            "/sandbox/",
            "--no-git-ignore",
        ];
        let (mut ok, _, mut err) = self
            .cli(&upload, Duration::from_secs(600))
            .await
            .map_err(rt)?;
        if !ok {
            // Removed outside Chummy? Make it again and try once more.
            self.made.lock().await.remove(&name);
            self.ensure(&name).await.map_err(rt)?;
            (ok, _, err) = self
                .cli(&upload, Duration::from_secs(600))
                .await
                .map_err(rt)?;
        }
        if !ok {
            return Err(rt(format!(
                "OpenShell couldn't copy the workspace in: {}",
                last_line(&err)
            )));
        }
        let remote = format!("/sandbox/{id}");
        let remote_ws = format!("{remote}/workspace");

        let outcome = {
            let mut cmd = clean_command(&program);
            let limit = (call.timeout + START_GRACE).as_secs().to_string();
            cmd.args([
                "sandbox",
                "exec",
                "-n",
                &name,
                "--no-tty",
                "--no-login-shell",
                "--workdir",
                &remote_ws,
                "--timeout",
                &limit,
            ]);
            let mut env = vec![
                ("CHUMMY_ROOT".to_string(), remote.clone()),
                ("CHUMMY_WORKSPACE".to_string(), remote_ws.clone()),
                ("HOME".to_string(), "/tmp".to_string()),
                ("PYTHONDONTWRITEBYTECODE".to_string(), "1".to_string()),
            ];
            env.extend(call.env.iter().cloned());
            for (k, v) in env {
                cmd.arg("--env").arg(format!("{k}={v}"));
            }
            let runner = format!("{remote}/chummy/_runner.py");
            cmd.args(["--", "python3", "-S", "-E", "-B", "-u", &runner]);
            process::run(
                Run {
                    cmd,
                    mode: &call.mode,
                    web: &self.web,
                    policy: &call.network,
                    timeout: call.timeout,
                    what: "OpenShell",
                },
                async {},
            )
            .await
        };

        // Bring changes back only from a call that finished.
        let result = match (outcome, call.jail, snapshot) {
            (Ok(mut out), Some(jail), Some(snap)) => {
                let back = local.join("back");
                std::fs::create_dir_all(&back).map_err(|e| rt(e.to_string()))?;
                let back_s = format!("{}/", back.display());
                let (ok, _, err) = self
                    .cli(
                        &["sandbox", "download", &name, &remote_ws, &back_s],
                        Duration::from_secs(600),
                    )
                    .await
                    .map_err(rt)?;
                if !ok {
                    Err(rt(format!(
                        "the tool ran, but OpenShell couldn't copy its changes back: {}",
                        last_line(&err)
                    )))
                } else {
                    // Downloading a folder into a folder puts it inside.
                    let inner = back.join("workspace");
                    let from = if inner.is_dir() { inner } else { back };
                    let report = sync::apply(jail, &snap, &from, LIMITS).map_err(rt)?;
                    if let Some(note) = report.note() {
                        out.log = if out.log.is_empty() {
                            note
                        } else {
                            format!("{}\n{note}", out.log)
                        };
                    }
                    Ok(out)
                }
            }
            (outcome, _, _) => outcome,
        };
        // Tidy the call folder in the sandbox (best effort).
        let _ = self
            .cli(
                &[
                    "sandbox",
                    "exec",
                    "-n",
                    &name,
                    "--no-tty",
                    "--no-login-shell",
                    "--",
                    "rm",
                    "-rf",
                    &remote,
                ],
                Duration::from_secs(60),
            )
            .await;
        result
    }
}

impl SandboxBackend for OpenShellSandbox {
    fn kind(&self) -> SandboxKind {
        SandboxKind::Openshell
    }

    fn check(&self) -> BoxFut<'_, Health> {
        Box::pin(self.health())
    }

    fn call<'a>(&'a self, call: SandboxCall<'a>) -> BoxFut<'a, Result<CallOutcome, ToolError>> {
        Box::pin(self.run_call(call))
    }
}

/// The sandbox for a workspace: one per folder, never shared.
pub fn sandbox_name(workspace: Option<&Path>) -> String {
    let key = match workspace {
        Some(p) => p.display().to_string(),
        None => String::new(),
    };
    let h = Sha256::digest(format!("{POLICY_VERSION}\n{key}"));
    format!("chummy-{POLICY_VERSION}-{}", &hex(&h)[..12])
}

fn call_id() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let h = Sha256::digest(format!("{}-{}-{n}", std::process::id(), super::now_ms()));
    format!("call-{}", &hex(&h)[..16])
}

/// Copies folders and regular files (links are left out).
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("couldn't copy the tool: {e}"))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("couldn't copy the tool: {e}"))? {
        let entry = entry.map_err(|e| format!("couldn't copy the tool: {e}"))?;
        let meta = std::fs::symlink_metadata(entry.path())
            .map_err(|e| format!("couldn't copy the tool: {e}"))?;
        let dest = to.join(entry.file_name());
        if meta.is_dir() {
            copy_tree(&entry.path(), &dest)?;
        } else if meta.is_file() {
            std::fs::copy(entry.path(), &dest)
                .map_err(|e| format!("couldn't copy the tool: {e}"))?;
        }
    }
    Ok(())
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_workspace_gets_its_own_sandbox() {
        let a = sandbox_name(Some(Path::new("/home/me/a")));
        let b = sandbox_name(Some(Path::new("/home/me/b")));
        assert_ne!(a, b);
        assert_eq!(a, sandbox_name(Some(Path::new("/home/me/a"))));
        assert!(a.starts_with("chummy-p1-") && a.len() <= 40, "{a}");
    }

    #[test]
    fn the_policy_allows_no_network() {
        assert!(!POLICY.lines().any(|l| l.starts_with("network_policies")));
        assert!(POLICY.contains("hard_requirement"));
        assert!(POLICY.contains("run_as_user: sandbox"));
        assert!(!POLICY.contains("- /\n"));
    }
}
