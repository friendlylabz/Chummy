//! The Python that tools run on: CPython compiled to WebAssembly (WASI).
//!
//! It's the official CPython WASI build (WASI is a tier 2 platform for
//! CPython), packaged by a CPython core developer. Chummy downloads it once,
//! checks its SHA-256 against the value pinned here, and keeps it under the
//! Chummy folder's `tools/`. It is never bundled with Chummy and never
//! installed system-wide.

use std::io::Read;
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ToolError;
use crate::lock::hex;

pub const PYTHON_VERSION: &str = "3.14.7";
const PYTHON_URL: &str = "https://github.com/brettcannon/cpython-wasi-build/releases/download/v3.14.7/python-3.14.7-wasi_sdk-24.zip";
const PYTHON_SHA256: &str = "2e064d3fb8172471d39d741348efa722349c40b96301f69968dff714999c584b";
/// Download size, for the confirmation dialog.
pub const PYTHON_DOWNLOAD_BYTES: u64 = 14_291_017;

/// Where tool files live, under the Chummy folder's `tools/`.
#[derive(Debug, Clone)]
pub struct ToolPaths {
    pub root: PathBuf,
    /// The WebAssembly Python: `python.wasm` and `lib/`.
    pub python: PathBuf,
    /// Chummy's runner and the `chummy` helper module.
    pub support: PathBuf,
    /// One folder per installed tool.
    pub installed: PathBuf,
    /// Compiled-code cache and uv's cache.
    pub cache: PathBuf,
}

impl ToolPaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            python: root.join(format!("python-{PYTHON_VERSION}")),
            support: root.join("support"),
            installed: root.join("installed"),
            cache: root.join("cache"),
            root,
        }
    }
    pub fn wasm(&self) -> PathBuf {
        self.python.join("python.wasm")
    }
    pub fn lib(&self) -> PathBuf {
        self.python.join("lib")
    }
    fn ready(&self) -> PathBuf {
        self.python.join("ready.json")
    }
    pub fn tool_dir(&self, name: &str) -> PathBuf {
        self.installed.join(name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PythonInfo {
    pub version: String,
    pub sha256: String,
}

/// The installed WebAssembly Python, if it's there and complete.
pub fn python_status(paths: &ToolPaths) -> Option<PythonInfo> {
    let info: PythonInfo = serde_json::from_slice(&std::fs::read(paths.ready()).ok()?).ok()?;
    (info.sha256 == PYTHON_SHA256 && paths.wasm().exists() && paths.lib().is_dir()).then_some(info)
}

/// Downloads, checks and unpacks the WebAssembly Python. `on` gets
/// (bytes so far, total).
pub async fn install_python(
    paths: &ToolPaths,
    http: &reqwest::Client,
    mut on: impl FnMut(u64, u64) + Send,
) -> Result<PythonInfo, ToolError> {
    if let Some(info) = python_status(paths) {
        return Ok(info);
    }
    std::fs::create_dir_all(&paths.root).map_err(ToolError::io("creating the tools folder"))?;
    let resp = http
        .get(PYTHON_URL)
        .send()
        .await
        .map_err(|e| ToolError::Network(chummy_http::reached("github.com".into(), &e)))?;
    if !resp.status().is_success() {
        return Err(ToolError::Runtime(format!(
            "github.com answered {} when downloading Python",
            resp.status().as_u16()
        )));
    }
    let total = resp.content_length().unwrap_or(PYTHON_DOWNLOAD_BYTES);
    let mut bytes = Vec::with_capacity(total as usize);
    let mut s = resp.bytes_stream();
    while let Some(chunk) = s.next().await {
        bytes.extend_from_slice(
            &chunk
                .map_err(|e| ToolError::Network(chummy_http::reached("github.com".into(), &e)))?,
        );
        on(bytes.len() as u64, total);
        if bytes.len() as u64 > 4 * PYTHON_DOWNLOAD_BYTES {
            return Err(ToolError::Runtime(
                "the Python download is far too large".into(),
            ));
        }
    }
    install_python_from(paths, &bytes)
}

/// Unpacks an already-downloaded archive (also used by tests).
pub fn install_python_from(paths: &ToolPaths, bytes: &[u8]) -> Result<PythonInfo, ToolError> {
    let sha = hex(&Sha256::digest(bytes));
    if sha != PYTHON_SHA256 {
        return Err(ToolError::Runtime(
            "the Python download didn't match its checksum, so it wasn't installed".into(),
        ));
    }
    let tmp = paths.root.join(".python-download");
    let _ = std::fs::remove_dir_all(&tmp);
    unpack(bytes, &tmp).map_err(|e| ToolError::Runtime(format!("couldn't unpack Python: {e}")))?;
    if !tmp.join("python.wasm").exists() || !tmp.join("lib").is_dir() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(ToolError::Runtime(
            "the Python download is missing python.wasm".into(),
        ));
    }
    let _ = std::fs::remove_dir_all(&paths.python);
    std::fs::rename(&tmp, &paths.python).map_err(ToolError::io("installing Python"))?;
    let info = PythonInfo {
        version: PYTHON_VERSION.into(),
        sha256: sha,
    };
    std::fs::write(
        paths.ready(),
        serde_json::to_vec_pretty(&info).unwrap_or_default(),
    )
    .map_err(ToolError::io("installing Python"))?;
    Ok(info)
}

fn unpack(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let rel = entry
            .enclosed_name()
            .ok_or_else(|| format!("unsafe path {:?}", entry.name().ok()))?;
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut data = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
        std::fs::write(&out, data).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Writes Chummy's runner and helper module (they ship inside Chummy and are
/// refreshed at every start, so they always match this version). Each file
/// is replaced atomically, and only if it changed, so a tool running at the
/// same time never sees a half-written file.
pub fn write_support(paths: &ToolPaths) -> Result<(), ToolError> {
    let pkg = paths.support.join("chummy");
    std::fs::create_dir_all(&pkg).map_err(ToolError::io("writing the tool runner"))?;
    for (path, text) in [
        (
            paths.support.join("_runner.py"),
            include_str!("../support/_runner.py"),
        ),
        (
            pkg.join("__init__.py"),
            include_str!("../support/chummy/__init__.py"),
        ),
    ] {
        if std::fs::read_to_string(&path).is_ok_and(|t| t == text) {
            continue;
        }
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = path.with_extension(format!("tmp-{}-{n}", std::process::id()));
        std::fs::write(&tmp, text)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .map_err(ToolError::io("writing the tool runner"))?;
    }
    Ok(())
}
