//! The built-in engine's Python runtime, installed on first use.
//!
//! Nothing here is bundled with Chummy. The first time a local model is run,
//! Chummy downloads `uv` (Astral's MIT/Apache-licensed Python package manager)
//! and uses it to install, inside the Chummy folder:
//!
//! - a private Python 3.12 (never the system Python);
//! - PyTorch, with `--torch-backend=auto` picking the CUDA build that matches
//!   the installed NVIDIA driver, or the CPU/Metal build otherwise;
//! - transformers, accelerate, safetensors and tokenizer helpers.
//!
//! Every version is pinned in [`crate::pins`] (`scripts/update-engine-pins.py`
//! moves them to the latest): uv is checked against a SHA-256 built into
//! Chummy, and the packages' dependencies are limited to releases from before
//! the pins were made, so a package published later can't sneak in.
//!
//! Removing the runtime is just deleting `runtime/`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::process::Command;

use crate::download::hex;
use crate::{LocalError, http_client, python_env, read_lines};

use crate::pins::{
    BITSANDBYTES, EXCLUDE_NEWER, PACKAGES, PYTHON_VERSION, TORCH, UV_ASSETS, UV_VERSION,
};

const UV_RELEASES: &str = "https://github.com/astral-sh/uv/releases/download";

/// Where the runtime lives, under the Chummy folder's `runtime/`.
#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub root: PathBuf,
    pub uv: PathBuf,
    pub venv: PathBuf,
    pub python: PathBuf,
    pub ready: PathBuf,
    pub engine_dir: PathBuf,
}

impl RuntimePaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let venv = root.join("venv");
        let python = if cfg!(windows) {
            venv.join("Scripts").join("python.exe")
        } else {
            venv.join("bin").join("python")
        };
        Self {
            uv: root
                .join("bin")
                .join(if cfg!(windows) { "uv.exe" } else { "uv" }),
            ready: root.join("ready.json"),
            engine_dir: root.join("engine"),
            python,
            venv,
            root,
        }
    }
}

/// Recorded once installation succeeds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeInfo {
    pub python: String,
    pub torch: String,
    pub transformers: String,
    /// "cuda", "mps" or "cpu".
    pub device: String,
    pub gpu: Option<String>,
    pub cuda: Option<String>,
    pub installed_at: u64,
}

/// The installed runtime, if installation finished.
pub fn status(paths: &RuntimePaths) -> Option<RuntimeInfo> {
    let info: RuntimeInfo = serde_json::from_slice(&std::fs::read(&paths.ready).ok()?).ok()?;
    paths.python.exists().then_some(info)
}

/// Deletes the runtime (models are kept).
pub fn remove(paths: &RuntimePaths) -> Result<(), LocalError> {
    if paths.root.exists() {
        std::fs::remove_dir_all(&paths.root).map_err(LocalError::io("removing the runtime"))?;
    }
    Ok(())
}

/// Rough size of what install will download, for the confirmation dialog.
pub fn approx_download_gb(has_nvidia: bool) -> f64 {
    if has_nvidia { 3.5 } else { 1.0 }
}

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub index: usize,
    pub count: usize,
    pub phase: &'static str,
    /// A line of installer output, if this update carries one.
    pub line: Option<String>,
}

const PHASES: [&str; 5] = [
    "Downloading the package manager",
    "Installing Python",
    "Installing PyTorch",
    "Installing transformers",
    "Checking the installation",
];

/// Installs (or repairs) the runtime. Safe to run again after a failure.
pub async fn install(
    paths: &RuntimePaths,
    cancel: &AtomicBool,
    mut on: impl FnMut(Step) + Send,
) -> Result<RuntimeInfo, LocalError> {
    let phase = |i: usize, on: &mut (dyn FnMut(Step) + Send)| {
        on(Step {
            index: i,
            count: PHASES.len(),
            phase: PHASES[i],
            line: None,
        })
    };
    let _ = std::fs::remove_file(&paths.ready);
    std::fs::create_dir_all(&paths.root).map_err(LocalError::io("creating the runtime folder"))?;

    phase(0, &mut on);
    if !paths.uv.exists() || uv_installed_version(paths).as_deref() != Some(UV_VERSION) {
        fetch_uv(paths).await?;
    }

    phase(1, &mut on);
    let venv = paths.venv.to_string_lossy().into_owned();
    run_uv(
        paths,
        &[
            "venv",
            "--allow-existing",
            "--python",
            PYTHON_VERSION,
            &venv,
        ],
        cancel,
        1,
        &mut on,
    )
    .await?;

    phase(2, &mut on);
    let py = paths.python.to_string_lossy().into_owned();
    run_uv(
        paths,
        &[
            "pip",
            "install",
            "--python",
            &py,
            "--torch-backend=auto",
            TORCH,
        ],
        cancel,
        2,
        &mut on,
    )
    .await?;

    phase(3, &mut on);
    let mut args = vec![
        "pip",
        "install",
        "--python",
        &py,
        "--exclude-newer",
        EXCLUDE_NEWER,
    ];
    args.extend_from_slice(PACKAGES);
    run_uv(paths, &args, cancel, 3, &mut on).await?;

    phase(4, &mut on);
    let info = probe(paths).await?;
    std::fs::write(
        &paths.ready,
        serde_json::to_vec_pretty(&info).unwrap_or_default(),
    )
    .map_err(LocalError::io("saving the runtime status"))?;
    Ok(info)
}

/// Installs bitsandbytes for 8-bit and 4-bit loading (NVIDIA only).
pub async fn ensure_bitsandbytes(
    paths: &RuntimePaths,
    cancel: &AtomicBool,
    mut on: impl FnMut(Step) + Send,
) -> Result<(), LocalError> {
    let py = paths.python.to_string_lossy().into_owned();
    run_uv(
        paths,
        &[
            "pip",
            "install",
            "--python",
            &py,
            "--exclude-newer",
            EXCLUDE_NEWER,
            BITSANDBYTES,
        ],
        cancel,
        3,
        &mut on,
    )
    .await
}

fn uv_asset() -> Result<&'static str, LocalError> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "uv-x86_64-unknown-linux-gnu.tar.gz",
        ("linux", "aarch64") => "uv-aarch64-unknown-linux-gnu.tar.gz",
        ("macos", "x86_64") => "uv-x86_64-apple-darwin.tar.gz",
        ("macos", "aarch64") => "uv-aarch64-apple-darwin.tar.gz",
        ("windows", "x86_64") => "uv-x86_64-pc-windows-msvc.zip",
        ("windows", "aarch64") => "uv-aarch64-pc-windows-msvc.zip",
        (os, arch) => {
            return Err(LocalError::Runtime(format!(
                "the built-in engine isn't available on {os}/{arch}"
            )));
        }
    })
}

async fn get_bytes(url: &str) -> Result<Vec<u8>, LocalError> {
    let resp = http_client()
        .get(url)
        .send()
        .await
        .map_err(|e| LocalError::Network(chummy_http::reached("github.com".into(), &e)))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(LocalError::Http {
            status,
            url: url.into(),
        });
    }
    let mut out = Vec::new();
    let mut s = resp.bytes_stream();
    while let Some(c) = s.next().await {
        out.extend_from_slice(
            &c.map_err(|e| LocalError::Network(chummy_http::reached("github.com".into(), &e)))?,
        );
    }
    Ok(out)
}

/// The pinned SHA-256 of a uv download.
fn uv_sha256(asset: &str) -> Option<&'static str> {
    UV_ASSETS
        .iter()
        .find(|(a, _)| *a == asset)
        .map(|(_, sha)| *sha)
}

fn uv_version_file(paths: &RuntimePaths) -> PathBuf {
    paths.root.join("bin").join("uv.version")
}

/// The uv version Chummy last installed, so a new pin replaces it.
fn uv_installed_version(paths: &RuntimePaths) -> Option<String> {
    let v = std::fs::read_to_string(uv_version_file(paths)).ok()?;
    Some(v.trim().to_string())
}

async fn fetch_uv(paths: &RuntimePaths) -> Result<(), LocalError> {
    let asset = uv_asset()?;
    let expected = uv_sha256(asset)
        .ok_or_else(|| LocalError::Runtime(format!("no pinned checksum for {asset}")))?;
    let archive = get_bytes(&format!("{UV_RELEASES}/{UV_VERSION}/{asset}")).await?;
    if hex(&Sha256::digest(&archive)) != expected {
        return Err(LocalError::Checksum { file: asset.into() });
    }

    let bin = paths.uv.parent().unwrap_or(&paths.root).to_path_buf();
    std::fs::create_dir_all(&bin).map_err(LocalError::io("creating the runtime folder"))?;
    let tmp = paths.root.join("uv-download");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(LocalError::io("unpacking uv"))?;

    if asset.ends_with(".zip") {
        let zip = tmp.join(asset);
        std::fs::write(&zip, &archive).map_err(LocalError::io("unpacking uv"))?;
        // Windows 10 and later ship bsdtar, which reads zip files.
        let tar = std::env::var_os("SystemRoot")
            .map(|r| PathBuf::from(r).join("System32").join("tar.exe"))
            .unwrap_or_else(|| "tar".into());
        let ok = quiet(Command::new(tar).arg("-xf").arg(&zip).arg("-C").arg(&tmp))
            .status()
            .await
            .map_err(LocalError::io("unpacking uv"))?
            .success();
        if !ok {
            return Err(LocalError::Runtime("couldn't unpack uv".into()));
        }
    } else {
        let gz = flate2::read::GzDecoder::new(&archive[..]);
        tar::Archive::new(gz)
            .unpack(&tmp)
            .map_err(LocalError::io("unpacking uv"))?;
    }

    let name = paths.uv.file_name().unwrap_or_default().to_owned();
    let found = find_file(&tmp, &name)
        .ok_or_else(|| LocalError::Runtime("uv wasn't in the download".into()))?;
    std::fs::rename(&found, &paths.uv)
        .or_else(|_| std::fs::copy(&found, &paths.uv).map(|_| ()))
        .map_err(LocalError::io("installing uv"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&paths.uv, std::fs::Permissions::from_mode(0o755));
    }
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::write(uv_version_file(paths), UV_VERSION).map_err(LocalError::io("installing uv"))?;
    Ok(())
}

fn find_file(dir: &Path, name: &std::ffi::OsStr) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = find_file(&p, name) {
                return Some(f);
            }
        } else if p.file_name() == Some(name) {
            return Some(p);
        }
    }
    None
}

/// No console window flashes on Windows.
pub(crate) fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

fn uv_command(paths: &RuntimePaths) -> Command {
    let mut c = Command::new(&paths.uv);
    c.env("UV_CACHE_DIR", paths.root.join("cache"))
        .env("UV_PYTHON_INSTALL_DIR", paths.root.join("python"))
        .env("UV_PYTHON_PREFERENCE", "only-managed")
        .env("UV_NO_CONFIG", "1")
        .env("NO_COLOR", "1")
        .env_remove("VIRTUAL_ENV")
        .env_remove("PYTHONPATH")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    quiet(&mut c);
    c
}

async fn run_uv(
    paths: &RuntimePaths,
    args: &[&str],
    cancel: &AtomicBool,
    index: usize,
    on: &mut (dyn FnMut(Step) + Send),
) -> Result<(), LocalError> {
    let mut child = uv_command(paths)
        .args(args)
        .spawn()
        .map_err(LocalError::io("starting uv"))?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    if let Some(out) = child.stdout.take() {
        let tx = tx.clone();
        read_lines(out, move |l| {
            let _ = tx.send(l);
        });
    }
    if let Some(err) = child.stderr.take() {
        let tx = tx.clone();
        read_lines(err, move |l| {
            let _ = tx.send(l);
        });
    }
    drop(tx);

    let mut tail = Vec::new();
    loop {
        tokio::select! {
            line = rx.recv() => match line {
                Some(l) => {
                    tail.push(l.clone());
                    if tail.len() > 20 { tail.remove(0); }
                    on(Step { index, count: PHASES.len(), phase: PHASES[index], line: Some(l) });
                }
                None => break,
            },
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if cancel.load(Ordering::Relaxed) {
                    let _ = child.kill().await;
                    return Err(LocalError::Cancelled);
                }
            }
        }
    }
    let status = child.wait().await.map_err(LocalError::io("running uv"))?;
    if !status.success() {
        return Err(LocalError::Runtime(format!(
            "{} failed:\n{}",
            PHASES[index].to_lowercase(),
            tail.join("\n")
        )));
    }
    Ok(())
}

const PROBE: &str = r#"
import json, platform, torch, transformers
dev = "cuda" if torch.cuda.is_available() else ("mps" if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available() else "cpu")
print(json.dumps({
  "python": platform.python_version(),
  "torch": torch.__version__,
  "transformers": transformers.__version__,
  "device": dev,
  "gpu": torch.cuda.get_device_name(0) if dev == "cuda" else None,
  "cuda": torch.version.cuda,
}))
"#;

async fn probe(paths: &RuntimePaths) -> Result<RuntimeInfo, LocalError> {
    let mut cmd = Command::new(&paths.python);
    cmd.arg("-c").arg(PROBE).stdin(Stdio::null());
    python_env(&mut cmd);
    quiet(&mut cmd);
    let out = cmd
        .output()
        .await
        .map_err(LocalError::io("checking the runtime"))?;
    if !out.status.success() {
        return Err(LocalError::Runtime(format!(
            "the runtime didn't start: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or_default()
        )));
    }
    let mut v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| LocalError::Runtime(format!("unexpected runtime check output: {e}")))?;
    v["installed_at"] = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .into();
    serde_json::from_value(v).map_err(|e| LocalError::Runtime(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_inside_the_runtime_folder() {
        let p = RuntimePaths::new("/x/runtime");
        assert!(p.python.starts_with("/x/runtime/venv"));
        assert!(p.uv.starts_with("/x/runtime/bin"));
        assert!(status(&p).is_none());
    }

    #[test]
    fn every_platform_has_a_pinned_checksum() {
        for asset in [
            "uv-x86_64-unknown-linux-gnu.tar.gz",
            "uv-aarch64-unknown-linux-gnu.tar.gz",
            "uv-x86_64-apple-darwin.tar.gz",
            "uv-aarch64-apple-darwin.tar.gz",
            "uv-x86_64-pc-windows-msvc.zip",
            "uv-aarch64-pc-windows-msvc.zip",
        ] {
            let sha = uv_sha256(asset).expect(asset);
            assert_eq!(sha.len(), 64);
            assert!(sha.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        if let Ok(asset) = uv_asset() {
            assert!(uv_sha256(asset).is_some());
        }
        for p in PACKAGES.iter().chain([&TORCH, &BITSANDBYTES]) {
            assert!(p.contains("=="), "{p} isn't pinned");
        }
        assert_eq!(PYTHON_VERSION.split('.').count(), 3);
    }

    #[test]
    fn there_is_a_uv_build_for_this_platform() {
        assert!(uv_asset().is_ok());
    }
}
