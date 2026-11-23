//! Running a tool as a child program (`docker run -i …`, `openshell
//! sandbox exec …`) that speaks the same line protocol as the WebAssembly
//! sandbox: the request goes in on stdin, the tool's web requests come out
//! on stdout and their answers go back in on stdin, and the last line is
//! the result. stderr is the tool's log.

use std::future::Future;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value as Json;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::ToolError;
use crate::host::{CallOutcome, LOG_LIMIT, Mode, STDOUT_LIMIT, answer, parse_done, request_json};
use crate::net::NetPolicy;

/// Extra time a program gets to start (a container takes a moment).
pub(crate) const START_GRACE: Duration = Duration::from_secs(15);

pub(crate) struct Run<'a> {
    pub cmd: Command,
    pub mode: &'a Mode,
    pub web: &'a reqwest::Client,
    pub policy: &'a NetPolicy,
    /// The tool's own time limit; [`START_GRACE`] is added.
    pub timeout: Duration,
    /// The program, for messages ("Docker").
    pub what: &'a str,
}

/// Runs `run.cmd` and talks to the tool. `on_timeout` runs when the time
/// is up, before the child is killed (to stop a container for good).
pub(crate) async fn run(
    mut run: Run<'_>,
    on_timeout: impl Future<Output = ()>,
) -> Result<CallOutcome, ToolError> {
    run.cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = run
        .cmd
        .spawn()
        .map_err(|e| ToolError::Runtime(format!("couldn't start {}: {e}", run.what)))?;
    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(ToolError::Runtime(format!("couldn't talk to {}", run.what)));
    };
    let log_task = tokio::spawn(read_capped(stderr, LOG_LIMIT));
    let started = Instant::now();
    let request = request_json(run.mode).to_string() + "\n";
    let (web, policy) = (run.web, run.policy);

    let talk = async {
        let mut done: Option<Json> = None;
        if stdin.write_all(request.as_bytes()).await.is_ok() && stdin.flush().await.is_ok() {
            let mut reader = BufReader::new(stdout.take(STDOUT_LIMIT as u64));
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let Ok(msg) = serde_json::from_slice::<Json>(&line) else {
                    continue;
                };
                if msg.get("op").and_then(|o| o.as_str()) == Some("done") {
                    done = Some(msg);
                    break;
                }
                let reply = answer(web, policy, &msg).await.to_string() + "\n";
                if stdin.write_all(reply.as_bytes()).await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        }
        drop(stdin);
        let status = child.wait().await.ok();
        (done, status)
    };
    let limit = run.timeout + START_GRACE;
    let outcome = tokio::time::timeout(limit, talk).await;
    let (done, status) = match outcome {
        Ok(x) => x,
        Err(_) => {
            on_timeout.await;
            let _ = child.kill().await;
            return Err(ToolError::Timeout(run.timeout.as_secs() as u32));
        }
    };
    let log = log_task.await.unwrap_or_default();
    if done.is_none() {
        if started.elapsed() >= run.timeout {
            return Err(ToolError::Timeout(run.timeout.as_secs() as u32));
        }
        let last = super::last_line(&log);
        let code = status.and_then(|s| s.code());
        // Docker and Podman use 125 for "couldn't run the container", and
        // 126/127 when the command (python3) is missing in the image.
        return Err(match code {
            Some(125..=127) => ToolError::Runtime(format!(
                "{} couldn't start the tool: {}",
                run.what,
                if last.is_empty() {
                    "no reason given"
                } else {
                    &last
                }
            )),
            _ if !last.is_empty() => ToolError::Crashed(last),
            Some(c) => ToolError::Crashed(format!("it stopped with exit code {c}")),
            None => ToolError::Crashed("it was stopped".into()),
        });
    }
    Ok(CallOutcome {
        result: parse_done(done)?,
        log,
    })
}

/// Reads everything, keeping the first `limit` bytes as text.
async fn read_capped(mut r: impl AsyncRead + Unpin, limit: usize) -> String {
    let mut kept = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = limit.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
    String::from_utf8_lossy(&kept).trim().to_string()
}

/// Runs a short command and returns (succeeded, stdout, stderr).
pub(crate) async fn output(
    mut cmd: Command,
    limit: Duration,
) -> Result<(bool, String, String), String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    match tokio::time::timeout(limit, cmd.output()).await {
        Err(_) => Err(format!(
            "it didn't answer within {} seconds",
            limit.as_secs()
        )),
        Ok(Err(e)) => Err(e.to_string()),
        Ok(Ok(o)) => Ok((
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).trim().to_string(),
            String::from_utf8_lossy(&o.stderr).trim().to_string(),
        )),
    }
}

/// A command with no inherited environment except what programs like
/// Docker need to find their configuration and daemon.
pub(crate) fn clean_command(program: &std::path::Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.env_clear();
    for key in [
        "PATH",
        "HOME",
        "USER",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
        "SYSTEMROOT",
        "SYSTEMDRIVE",
        "TEMP",
        "TMP",
        "TMPDIR",
        "XDG_RUNTIME_DIR",
        "XDG_CONFIG_HOME",
        "DOCKER_HOST",
        "DOCKER_CONTEXT",
        "DOCKER_CONFIG",
        "DOCKER_CERT_PATH",
        "DOCKER_TLS_VERIFY",
        "CONTAINER_HOST",
        "CONTAINER_CONNECTION",
        "CONTAINERS_CONF",
        "OPENSHELL_GATEWAY",
        "OPENSHELL_CONFIG_DIR",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "NO_PROXY",
        "https_proxy",
        "http_proxy",
        "no_proxy",
    ] {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    // Programs started from a desktop app often have a short PATH; make
    // sure the program's own folder is on it (Docker Desktop's helpers).
    if let Some(dir) = program.parent() {
        let mut paths: Vec<_> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        if !paths.iter().any(|p| p == dir) {
            paths.push(dir.to_path_buf());
            if let Ok(joined) = std::env::join_paths(paths) {
                cmd.env("PATH", joined);
            }
        }
    }
    #[cfg(windows)]
    {
        // No console window flashing up for each tool call.
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}
