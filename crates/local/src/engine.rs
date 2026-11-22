//! Running `chummy_engine.py` as a child process.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chummy_llm::Precision;
use serde::Serialize;
use tokio::process::{Child, Command};

use crate::runtime::quiet;
use crate::{ENGINE_SCRIPT, LocalError, http_client, python_env, read_lines};

/// How long a model may take to load before we give up.
const LOAD_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const LOG_LINES: usize = 300;

/// What to run.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineConfig {
    pub model_dir: PathBuf,
    pub parser: String,
    pub precision: Precision,
    /// "auto", "cuda", "mps" or "cpu".
    pub device: String,
    /// Extra arguments (tests pass `--fake`).
    pub extra_args: Vec<String>,
}

/// Where the running engine listens. The token must be sent as a bearer token.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Endpoint {
    pub base_url: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EngineStatus {
    Stopped,
    Loading { model: String },
    Ready { model: String, device: String },
    Failed { error: String },
}

struct Running {
    child: Child,
    cfg: EngineConfig,
    endpoint: Endpoint,
}

/// Owns at most one engine process.
pub struct Engine {
    python: PathBuf,
    script_dir: PathBuf,
    running: tokio::sync::Mutex<Option<Running>>,
    status: Arc<Mutex<EngineStatus>>,
    log: Arc<Mutex<VecDeque<String>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn model_name(cfg: &EngineConfig) -> String {
    cfg.model_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl Engine {
    /// `python` is the runtime's interpreter; the script is written to `script_dir`.
    pub fn new(python: impl Into<PathBuf>, script_dir: impl Into<PathBuf>) -> Self {
        Self {
            python: python.into(),
            script_dir: script_dir.into(),
            running: tokio::sync::Mutex::new(None),
            status: Arc::new(Mutex::new(EngineStatus::Stopped)),
            log: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn status(&self) -> EngineStatus {
        lock(&self.status).clone()
    }

    pub fn log_tail(&self) -> Vec<String> {
        lock(&self.log).iter().cloned().collect()
    }

    fn set_status(&self, s: EngineStatus) {
        *lock(&self.status) = s;
    }

    /// Returns the endpoint of an engine running `cfg`, starting (or
    /// restarting) one if needed. `on_status` hears every status change.
    pub async fn ensure(
        &self,
        cfg: &EngineConfig,
        mut on_status: impl FnMut(&EngineStatus),
    ) -> Result<Endpoint, LocalError> {
        let mut guard = self.running.lock().await;
        if let Some(r) = guard.as_mut() {
            let alive = matches!(r.child.try_wait(), Ok(None));
            if alive && r.cfg == *cfg && matches!(self.status(), EngineStatus::Ready { .. }) {
                return Ok(r.endpoint.clone());
            }
            let _ = r.child.kill().await;
            *guard = None;
        }

        let script = self.write_script()?;
        let port = free_port()?;
        let token = random_token()?;
        let endpoint = Endpoint {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            token: token.clone(),
        };
        let model = model_name(cfg);

        lock(&self.log).clear();
        let mut cmd = Command::new(&self.python);
        cmd.arg("-u")
            .arg(&script)
            .arg("--model")
            .arg(&cfg.model_dir)
            .arg("--parser")
            .arg(&cfg.parser)
            .arg("--port")
            .arg(port.to_string())
            .arg("--precision")
            .arg(match cfg.precision {
                Precision::Half => "half",
                Precision::Int8 => "int8",
                Precision::Int4 => "int4",
            })
            .arg("--device")
            .arg(&cfg.device)
            .args(&cfg.extra_args)
            .env("CHUMMY_ENGINE_TOKEN", &token)
            // Never touch the network at run time.
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        python_env(&mut cmd);
        quiet(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(LocalError::io("starting the local engine"))?;

        let log_line = |log: Arc<Mutex<VecDeque<String>>>| {
            move |l: String| {
                let mut log = lock(&log);
                if log.len() >= LOG_LINES {
                    log.pop_front();
                }
                log.push_back(l);
            }
        };
        if let Some(out) = child.stdout.take() {
            read_lines(out, log_line(self.log.clone()));
        }
        if let Some(err) = child.stderr.take() {
            read_lines(err, log_line(self.log.clone()));
        }

        let s = EngineStatus::Loading {
            model: model.clone(),
        };
        self.set_status(s.clone());
        on_status(&s);

        let http = http_client();
        let started = Instant::now();
        loop {
            if let Ok(Some(exit)) = child.try_wait() {
                tokio::time::sleep(Duration::from_millis(100)).await; // let log readers finish
                let tail = self
                    .log_tail()
                    .into_iter()
                    .rev()
                    .take(5)
                    .collect::<Vec<_>>();
                let error = format!(
                    "the engine stopped ({exit}): {}",
                    tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
                );
                return Err(self.fail(error, &mut on_status));
            }
            if started.elapsed() > LOAD_TIMEOUT {
                let _ = child.kill().await;
                return Err(self.fail("the model took too long to load".into(), &mut on_status));
            }
            let health = http
                .get(format!("http://127.0.0.1:{port}/health"))
                .bearer_auth(&token)
                .timeout(Duration::from_secs(2))
                .send()
                .await;
            if let Ok(resp) = health
                && let Ok(v) = resp.json::<serde_json::Value>().await
            {
                match v["status"].as_str() {
                    Some("ready") => {
                        let s = EngineStatus::Ready {
                            model,
                            device: v["device"].as_str().unwrap_or("cpu").into(),
                        };
                        self.set_status(s.clone());
                        on_status(&s);
                        *guard = Some(Running {
                            child,
                            cfg: cfg.clone(),
                            endpoint: endpoint.clone(),
                        });
                        return Ok(endpoint);
                    }
                    Some("error") => {
                        let _ = child.kill().await;
                        let error = v["error"]
                            .as_str()
                            .unwrap_or("the model failed to load")
                            .to_string();
                        return Err(self.fail(error, &mut on_status));
                    }
                    _ => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }

    fn fail(&self, error: String, on_status: &mut impl FnMut(&EngineStatus)) -> LocalError {
        let s = EngineStatus::Failed {
            error: error.clone(),
        };
        self.set_status(s.clone());
        on_status(&s);
        LocalError::Runtime(error)
    }

    /// Stops the engine if it is running.
    pub async fn stop(&self) {
        if let Some(mut r) = self.running.lock().await.take() {
            let _ = r.child.kill().await;
        }
        self.set_status(EngineStatus::Stopped);
    }

    fn write_script(&self) -> Result<PathBuf, LocalError> {
        std::fs::create_dir_all(&self.script_dir)
            .map_err(LocalError::io("preparing the engine"))?;
        let path = self.script_dir.join("chummy_engine.py");
        if std::fs::read_to_string(&path).ok().as_deref() != Some(ENGINE_SCRIPT) {
            std::fs::write(&path, ENGINE_SCRIPT).map_err(LocalError::io("preparing the engine"))?;
        }
        Ok(path)
    }
}

fn free_port() -> Result<u16, LocalError> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(LocalError::io("finding a free port"))?;
    Ok(l.local_addr()
        .map_err(LocalError::io("finding a free port"))?
        .port())
}

fn random_token() -> Result<String, LocalError> {
    let mut b = [0u8; 24];
    getrandom::fill(&mut b).map_err(|e| LocalError::Runtime(format!("no randomness: {e}")))?;
    Ok(crate::download::hex(&b))
}

/// The model folder for a downloaded repo.
pub fn model_dir(models_dir: &Path, dir_name: &str) -> PathBuf {
    models_dir.join(dir_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chummy_llm::{Client, ProviderConfig, ProviderKind, StepStatus, probe};

    fn system_python() -> Option<&'static str> {
        ["python3", "python"].into_iter().find(|p| {
            std::process::Command::new(p)
                .arg("--version")
                .output()
                .is_ok_and(|o| o.status.success())
        })
    }

    /// The whole pipeline against the engine's fake backend: process start,
    /// token auth, health wait, streaming, tool-call parsing and the probe.
    #[tokio::test]
    async fn fake_engine_passes_the_connection_probe() {
        let Some(py) = system_python() else {
            eprintln!("skipping: no python on PATH");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let engine = Engine::new(py, tmp.path().join("engine"));
        let cfg = EngineConfig {
            model_dir: tmp.path().join("org--fake"),
            parser: "hermes".into(),
            precision: Precision::Half,
            device: "cpu".into(),
            extra_args: vec!["--fake".into()],
        };
        let mut seen = vec![];
        let ep = engine.ensure(&cfg, |s| seen.push(s.clone())).await.unwrap();
        assert!(matches!(seen.last(), Some(EngineStatus::Ready { .. })));

        // Same config: reuses the running process.
        let again = engine.ensure(&cfg, |_| {}).await.unwrap();
        assert_eq!(ep, again);

        let provider = ProviderConfig {
            id: "local".into(),
            name: "Local".into(),
            kind: ProviderKind::Local,
            base_url: ep.base_url.clone(),
            model: "fake-model".into(),
            local: None,
            temperature: None,
        };
        let report = probe(&Client::new(&provider, Some(ep.token.clone())).unwrap()).await;
        assert!(report.ok, "{:?}", report.steps);
        assert!(
            report.steps.iter().all(|s| s.status == StepStatus::Pass),
            "{:?}",
            report.steps
        );

        // Wrong token is refused.
        let bad = probe(&Client::new(&provider, Some("nope".into())).unwrap()).await;
        assert!(!bad.ok);

        engine.stop().await;
        assert_eq!(engine.status(), EngineStatus::Stopped);
    }
}
