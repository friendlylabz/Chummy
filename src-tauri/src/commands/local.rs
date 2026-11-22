//! The built-in local engine: inspecting, downloading, runtime setup, running.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use chummy_llm::{LocalModel, Precision, ProviderConfig, ProviderKind};
use chummy_local::download::{self, Installed, Progress, Request};
use chummy_local::engine::{Endpoint, EngineConfig, EngineStatus};
use chummy_local::hardware::Hardware;
use chummy_local::hf::{Hub, Inspection, dir_name};
use chummy_local::runtime::{self, RuntimeInfo, Step};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

use super::{CmdResult, err};
use crate::dialogs;
use crate::events;
use crate::state::{AppState, HF_TOKEN_KEY, lock};

fn hardware() -> &'static Hardware {
    static HW: OnceLock<Hardware> = OnceLock::new();
    HW.get_or_init(Hardware::detect)
}

fn hub(state: &AppState) -> Hub {
    Hub::new(state.secrets.get(HF_TOKEN_KEY).ok().flatten())
}

#[derive(Serialize)]
pub struct LocalStatus {
    pub runtime: Option<RuntimeInfo>,
    pub runtime_installing: bool,
    pub runtime_download_gb: f64,
    pub hardware: Hardware,
    pub hardware_summary: String,
    pub engine: EngineStatus,
    pub models: Vec<Installed>,
    pub downloading: Vec<String>,
    pub hf_token_set: bool,
    pub device: String,
}

#[tauri::command]
pub fn local_status(state: State<'_, AppState>) -> LocalStatus {
    let hw = hardware().clone();
    LocalStatus {
        runtime: runtime::status(&state.runtime),
        runtime_installing: lock(&state.runtime_install).is_some(),
        runtime_download_gb: runtime::approx_download_gb(!hw.nvidia.is_empty()),
        hardware_summary: hw.summary(),
        hardware: hw,
        engine: state.engine.status(),
        models: download::installed(&state.dirs.models),
        downloading: lock(&state.downloads).keys().cloned().collect(),
        hf_token_set: state.has_secret(HF_TOKEN_KEY),
        device: state.settings().local.device,
    }
}

/// Checks a Hugging Face repo and estimates what it needs. Downloads nothing.
#[tauri::command]
pub async fn inspect_model(state: State<'_, AppState>, repo: String) -> CmdResult<Inspection> {
    hub(&state).inspect(&repo, hardware()).await.map_err(err)
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DownloadEvent {
    Progress { progress: Progress },
    Done { repo: String, provider_id: String },
    Failed { repo: String, error: String },
    Cancelled { repo: String },
}

/// Starts downloading a model. The repo is inspected again here, so only a
/// model that passes every check is ever downloaded, and only the files the
/// inspection chose. Progress arrives as `local:download` events. When the
/// download finishes, the model is added as a connection.
#[tauri::command]
pub async fn download_model(
    app: AppHandle,
    state: State<'_, AppState>,
    repo: String,
    precision: Precision,
) -> CmdResult<()> {
    let hub = hub(&state);
    let inspection = hub.inspect(&repo, hardware()).await.map_err(err)?;
    if !inspection.ok {
        let reasons: Vec<String> = inspection
            .checks
            .iter()
            .filter(|c| c.status == chummy_local::hf::CheckStatus::Fail)
            .map(|c| format!("{}: {}", c.label, c.detail))
            .collect();
        return Err(format!(
            "This model can't be used with the built-in engine.\n{}",
            reasons.join("\n")
        ));
    }
    if precision != Precision::Half && hardware().nvidia.is_empty() {
        return Err("8-bit and 4-bit need an NVIDIA GPU.".into());
    }

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut d = lock(&state.downloads);
        if d.contains_key(&inspection.repo) {
            return Err("That model is already downloading.".into());
        }
        d.insert(inspection.repo.clone(), cancel.clone());
    }

    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let repo = inspection.repo.clone();
        let parser = inspection.parser.clone().unwrap_or_default();
        let req = Request {
            repo: &repo,
            revision: &inspection.revision,
            parser: &parser,
            files: &inspection.files,
        };
        let result = download::download(&hub, req, &state.dirs.models, &cancel, |p| {
            let _ = app.emit(
                events::LOCAL_DOWNLOAD,
                DownloadEvent::Progress {
                    progress: p.clone(),
                },
            );
        })
        .await;
        lock(&state.downloads).remove(&repo);

        let ev = match result {
            Ok(_) => match add_local_provider(&app, &state, &inspection, &parser, precision) {
                Ok(provider_id) => DownloadEvent::Done {
                    repo: repo.clone(),
                    provider_id,
                },
                Err(error) => DownloadEvent::Failed {
                    repo: repo.clone(),
                    error,
                },
            },
            Err(chummy_local::LocalError::Cancelled) => {
                DownloadEvent::Cancelled { repo: repo.clone() }
            }
            Err(e) => DownloadEvent::Failed {
                repo: repo.clone(),
                error: e.to_string(),
            },
        };
        let _ = app.emit(events::LOCAL_DOWNLOAD, ev);
    });
    Ok(())
}

fn add_local_provider(
    app: &AppHandle,
    state: &AppState,
    i: &Inspection,
    parser: &str,
    precision: Precision,
) -> Result<String, String> {
    let dir = dir_name(&i.repo);
    let id = format!("local-{}", dir.to_lowercase());
    let name = i.repo.split('/').nth(1).unwrap_or(&i.repo).to_string();
    let cfg = ProviderConfig {
        id: id.clone(),
        name,
        kind: ProviderKind::Local,
        base_url: String::new(),
        model: i.repo.clone(),
        local: Some(LocalModel {
            repo: i.repo.clone(),
            revision: i.revision.clone(),
            dir_name: dir,
            parser: parser.into(),
            precision,
        }),
        temperature: None,
        daily_tokens: Some(chummy_llm::DEFAULT_DAILY_TOKENS),
    };
    state.update(app, |s| {
        match s.providers.iter_mut().find(|p| p.id == id) {
            Some(p) => *p = cfg,
            None => s.providers.push(cfg),
        }
        if s.default_provider.is_none() {
            s.default_provider = Some(id.clone());
        }
    })?;
    Ok(id)
}

#[tauri::command]
pub fn cancel_download(state: State<'_, AppState>, repo: String) {
    if let Some(flag) = lock(&state.downloads).get(&repo) {
        flag.store(true, Ordering::Relaxed);
    }
}

/// Deletes a downloaded model's files (after a native confirmation) and any
/// connection that uses it.
#[tauri::command]
pub async fn delete_local_model(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    dir_name: String,
) -> CmdResult<bool> {
    let msg = format!(
        "The downloaded files for {dir_name} will be deleted. You can download it again later."
    );
    if !dialogs::confirm(
        &app,
        window.label(),
        "Delete this model?",
        &msg,
        "Delete",
        true,
    )
    .await
    {
        return Ok(false);
    }
    state.engine.stop().await;
    download::remove(&state.dirs.models, &dir_name).map_err(err)?;
    state.update(&app, |s| {
        s.providers
            .retain(|p| p.local.as_ref().is_none_or(|l| l.dir_name != dir_name));
        if s.default_provider
            .as_ref()
            .is_some_and(|d| s.provider(d).is_none())
        {
            s.default_provider = s.providers.first().map(|p| p.id.clone());
        }
    })?;
    Ok(true)
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeEvent {
    Step { step: Step },
    Done { info: RuntimeInfo },
    Failed { error: String },
    Cancelled,
}

/// Installs Python, PyTorch and transformers for the built-in engine, after
/// a native confirmation that states the download size.
#[tauri::command]
pub async fn install_runtime(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<bool> {
    if lock(&state.runtime_install).is_some() {
        return Ok(true);
    }
    let hw = hardware();
    let gb = runtime::approx_download_gb(!hw.nvidia.is_empty());
    let gpu = if hw.nvidia.is_empty() {
        if hw.apple_silicon {
            "Apple Silicon (Metal)".to_string()
        } else {
            "CPU only (no NVIDIA GPU found)".to_string()
        }
    } else {
        format!("{} (CUDA)", hw.nvidia[0].name)
    };
    let msg = format!(
        "The built-in engine needs Python, PyTorch and Hugging Face transformers. Chummy will download \
         about {gb:.1} GB and install them inside its own folder, set up for: {gpu}.\n\nNothing is \
         installed system-wide, and you can remove it later in Settings."
    );
    if !dialogs::confirm(
        &app,
        window.label(),
        "Set up the local engine?",
        &msg,
        "Download and install",
        false,
    )
    .await
    {
        return Ok(false);
    }

    let cancel = Arc::new(AtomicBool::new(false));
    *lock(&state.runtime_install) = Some(cancel.clone());
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let result = runtime::install(&state.runtime, &cancel, |step| {
            let _ = app.emit(events::LOCAL_RUNTIME, RuntimeEvent::Step { step });
        })
        .await;
        *lock(&state.runtime_install) = None;
        let ev = match result {
            Ok(info) => RuntimeEvent::Done { info },
            Err(chummy_local::LocalError::Cancelled) => RuntimeEvent::Cancelled,
            Err(e) => RuntimeEvent::Failed {
                error: e.to_string(),
            },
        };
        let _ = app.emit(events::LOCAL_RUNTIME, ev);
    });
    Ok(true)
}

#[tauri::command]
pub fn cancel_runtime_install(state: State<'_, AppState>) {
    if let Some(flag) = lock(&state.runtime_install).as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub async fn remove_runtime(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<bool> {
    let msg = "Python, PyTorch and transformers will be removed from the Chummy folder. Downloaded \
               models are kept. You can set the engine up again at any time.";
    if !dialogs::confirm(
        &app,
        window.label(),
        "Remove the local engine?",
        msg,
        "Remove",
        true,
    )
    .await
    {
        return Ok(false);
    }
    state.engine.stop().await;
    runtime::remove(&state.runtime).map_err(err)?;
    Ok(true)
}

/// Stores (or with `None`/empty, removes) the Hugging Face token in the keychain.
#[tauri::command]
pub fn set_hf_token(
    app: AppHandle,
    state: State<'_, AppState>,
    token: Option<String>,
) -> CmdResult<()> {
    match token.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => state.secrets.set(HF_TOKEN_KEY, t).map_err(err)?,
        None => state.secrets.delete(HF_TOKEN_KEY).map_err(err)?,
    }
    state.update(&app, |_| ()) // re-broadcast so windows see hf_token_set
}

#[tauri::command]
pub async fn set_local_device(
    app: AppHandle,
    state: State<'_, AppState>,
    device: String,
) -> CmdResult<()> {
    if !matches!(device.as_str(), "auto" | "cuda" | "mps" | "cpu") {
        return Err("Unknown device.".into());
    }
    state.engine.stop().await;
    let _ = app.emit(events::LOCAL_ENGINE, EngineStatus::Stopped);
    state.update(&app, |s| s.local.device = device)
}

#[tauri::command]
pub async fn stop_engine(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    state.engine.stop().await;
    let _ = app.emit(events::LOCAL_ENGINE, EngineStatus::Stopped);
    Ok(())
}

#[tauri::command]
pub fn engine_log(state: State<'_, AppState>) -> Vec<String> {
    state.engine.log_tail()
}

/// Starts (or reuses) the engine for a local provider and returns its endpoint.
pub async fn start_engine_for(
    app: &AppHandle,
    state: &AppState,
    provider: &ProviderConfig,
) -> Result<Endpoint, String> {
    let local = provider
        .local
        .as_ref()
        .ok_or("This connection isn't a local model.")?;
    let model_dir = state.dirs.models.join(&local.dir_name);
    if !model_dir.join(download::MANIFEST).exists() {
        return Err(
            "This model's files are missing. Download it again in Settings → Local engine.".into(),
        );
    }
    if local.precision != Precision::Half {
        let never = AtomicBool::new(false);
        runtime::ensure_bitsandbytes(&state.runtime, &never, |_| {})
            .await
            .map_err(err)?;
    }
    let cfg = EngineConfig {
        model_dir,
        parser: local.parser.clone(),
        precision: local.precision,
        device: state.settings().local.device,
        extra_args: vec![],
    };
    state
        .engine
        .ensure(&cfg, |s| {
            let _ = app.emit(events::LOCAL_ENGINE, s.clone());
        })
        .await
        .map_err(|e| {
            let e = e.to_string();
            chummy_log::system::log(
                chummy_log::system::Level::Error,
                "engine",
                format!("The built-in engine couldn't start {}", provider.name),
                &[("error", &e)],
            );
            if e.contains("ModuleNotFoundError") || e.contains("ImportError") {
                format!("{e}\n\nThe local engine's setup looks incomplete. Use Repair in Settings → Local engine.")
            } else {
                e
            }
        })
}
