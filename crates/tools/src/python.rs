//! The Python that tools run on: CPython compiled to WebAssembly (WASI).
//!
//! Chummy's release workflow builds it from python.org's signed source with
//! CPython's own WASI build (`scripts/build-wasi-python.sh`; WASI is a tier
//! 2 platform for CPython) and bundles the result in the app. Setting up
//! tools unpacks it under the Chummy folder's `tools/`: nothing is
//! downloaded, and nothing is installed system-wide.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ToolError;
use crate::lock::hex;

/// The version `scripts/build-wasi-python.sh` builds (a test checks they match).
pub const PYTHON_VERSION: &str = "3.14.10";

/// "3.14" of "3.14.10".
pub fn minor_version(v: &str) -> &str {
    match v.match_indices('.').nth(1) {
        Some((i, _)) => &v[..i],
        None => v,
    }
}

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
    (info.version == PYTHON_VERSION && paths.wasm().exists() && paths.lib().is_dir())
        .then_some(info)
}

/// Unpacks the WebAssembly Python from its zip (bundled in the app; tests
/// pass a built one), replacing any installed copy. Blocking.
pub fn install_python_from(paths: &ToolPaths, bytes: &[u8]) -> Result<PythonInfo, ToolError> {
    std::fs::create_dir_all(&paths.root).map_err(ToolError::io("creating the tools folder"))?;
    let sha = hex(&Sha256::digest(bytes));
    let tmp = paths.root.join(".python-unpack");
    let _ = std::fs::remove_dir_all(&tmp);
    unpack(bytes, &tmp).map_err(|e| ToolError::Runtime(format!("couldn't unpack Python: {e}")))?;
    let stdlib = tmp
        .join("lib")
        .join(format!("python{}", minor_version(PYTHON_VERSION)));
    if !tmp.join("python.wasm").exists() || !stdlib.is_dir() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(ToolError::Runtime(format!(
            "the bundled Python isn't Python {PYTHON_VERSION} for WebAssembly"
        )));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_the_one_the_build_script_builds() {
        let script = include_str!("../../../scripts/build-wasi-python.sh");
        assert!(
            script.contains(&format!("PY_VERSION=\"{PYTHON_VERSION}\"")),
            "scripts/build-wasi-python.sh builds a different Python than PYTHON_VERSION"
        );
    }

    #[test]
    fn minor_versions() {
        assert_eq!(minor_version("3.14.10"), "3.14");
        assert_eq!(minor_version("3.14"), "3.14");
    }

    #[test]
    fn a_zip_without_python_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = ToolPaths::new(tmp.path());
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            z.start_file("python.wasm", zip::write::SimpleFileOptions::default())
                .unwrap();
            z.finish().unwrap();
        }
        let e = install_python_from(&paths, buf.get_ref()).unwrap_err();
        assert!(e.to_string().contains("isn't Python"), "{e}");
        assert!(python_status(&paths).is_none());
    }
}
