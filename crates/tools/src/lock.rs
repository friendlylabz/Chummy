//! Dependencies: resolving them to a lock, and installing a lock.
//!
//! Locks use the standard [PEP 751] `pylock.toml` format. Every package must
//! come as a pure-Python wheel (`…-none-any.whl`) with a SHA-256 hash, since
//! compiled code can't run in the WebAssembly sandbox. Installing a pure
//! wheel is just unpacking it, so Chummy does that itself: no Python on the
//! computer is needed, and nothing from a package runs during installation.
//!
//! Built-in tools ship with their locks. For other tools, [`resolve`] asks
//! `uv` to produce one.
//!
//! [PEP 751]: https://peps.python.org/pep-0751/

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ToolError;
use crate::python::PYTHON_VERSION;

/// Largest wheel Chummy will download.
const MAX_WHEEL: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct PyLock {
    #[serde(rename = "lock-version")]
    pub lock_version: String,
    #[serde(rename = "requires-python")]
    pub requires_python: Option<String>,
    #[serde(default)]
    pub packages: Vec<LockPackage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LockPackage {
    pub name: String,
    pub version: Option<String>,
    /// Environment markers. Locks for the sandbox are resolved for one
    /// platform and shouldn't need any, so a lock with one is refused rather
    /// than guessed at.
    pub marker: Option<String>,
    #[serde(default)]
    pub wheels: Vec<LockWheel>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LockWheel {
    pub url: Option<String>,
    pub name: Option<String>,
    pub size: Option<u64>,
    #[serde(default)]
    pub hashes: BTreeMap<String, String>,
}

/// An installed package, recorded in the tool's manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
}

impl PyLock {
    pub fn parse(text: &str) -> Result<Self, ToolError> {
        let lock: PyLock = toml::from_str(text).map_err(|e| {
            ToolError::Dependencies(format!("the lock file isn't valid: {}", e.message()))
        })?;
        if let Some(req) = &lock.requires_python
            && !crate::meta::python_satisfies(req, PYTHON_VERSION).unwrap_or(false)
        {
            return Err(ToolError::Dependencies(format!(
                "the lock is for Python {req}, and tools run on Python {PYTHON_VERSION}"
            )));
        }
        if let Some(p) = lock.packages.iter().find(|p| p.marker.is_some()) {
            return Err(ToolError::Dependencies(format!(
                "the lock has an environment marker for {} (locks for the sandbox can't have markers)",
                p.name
            )));
        }
        if !lock.lock_version.starts_with("1.") {
            return Err(ToolError::Dependencies(format!(
                "lock version {} isn't supported",
                lock.lock_version
            )));
        }
        Ok(lock)
    }
}

/// The file name's tags, if it's a wheel: (python, abi, platform).
fn wheel_tags(file: &str) -> Option<(String, String, String)> {
    let stem = file.strip_suffix(".whl")?;
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() < 5 {
        return None;
    }
    let n = parts.len();
    Some((
        parts[n - 3].to_string(),
        parts[n - 2].to_string(),
        parts[n - 1].to_string(),
    ))
}

/// Pure-Python wheels for Python 3: no compiled code, any platform.
pub(crate) fn is_pure_wheel(file: &str) -> bool {
    wheel_tags(file).is_some_and(|(py, abi, plat)| {
        abi == "none" && plat == "any" && py.split('.').any(|t| t == "py3" || t.starts_with("py3"))
    })
}

fn wheel_file_name(w: &LockWheel) -> Option<String> {
    w.name.clone().or_else(|| {
        w.url
            .as_deref()
            .and_then(|u| u.split('?').next())
            .and_then(|u| u.rsplit('/').next())
            .map(str::to_string)
    })
}

/// Picks the wheel to install for each package, or explains why it can't.
fn plan(lock: &PyLock) -> Result<Vec<(String, String, String, String)>, ToolError> {
    let mut out = Vec::new();
    for p in &lock.packages {
        let pick = p.wheels.iter().find_map(|w| {
            let file = wheel_file_name(w)?;
            let url = w.url.clone()?;
            let sha = w.hashes.get("sha256")?.to_lowercase();
            (is_pure_wheel(&file) && url.starts_with("https://")).then_some((file, url, sha))
        });
        let Some((file, url, sha)) = pick else {
            return Err(ToolError::Dependencies(format!(
                "{} has no pure-Python wheel with a SHA-256 hash, so it can't run in the sandbox \
                 (packages with compiled code aren't supported)",
                p.name
            )));
        };
        out.push((
            p.name.clone(),
            p.version.clone().unwrap_or_default(),
            file,
            url + "#" + &sha,
        ));
    }
    Ok(out)
}

/// Downloads and unpacks every package in `lock` into `site_packages`.
pub async fn install_lock(
    lock: &PyLock,
    site_packages: &Path,
    http: &reqwest::Client,
    on: &mut (dyn FnMut(&str) + Send),
) -> Result<Vec<Package>, ToolError> {
    let steps = plan(lock)?;
    std::fs::create_dir_all(site_packages).map_err(ToolError::io("creating the package folder"))?;
    let mut installed = Vec::new();
    for (name, version, file, url_and_sha) in steps {
        let (url, sha) = url_and_sha.rsplit_once('#').unwrap_or((&url_and_sha, ""));
        on(&format!("Downloading {file}"));
        let bytes = download(http, url).await?;
        if hex(&Sha256::digest(&bytes)) != sha {
            return Err(ToolError::Dependencies(format!(
                "{file} didn't match its checksum, so it wasn't installed"
            )));
        }
        unpack_wheel(&bytes, site_packages)
            .map_err(|e| ToolError::Dependencies(format!("couldn't unpack {file}: {e}")))?;
        installed.push(Package { name, version });
    }
    Ok(installed)
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

async fn download(http: &reqwest::Client, url: &str) -> Result<Vec<u8>, ToolError> {
    let host = url.split('/').nth(2).unwrap_or(url).to_string();
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| ToolError::Network(chummy_http::reached(host.clone(), &e)))?;
    if !resp.status().is_success() {
        return Err(ToolError::Dependencies(format!(
            "{host} answered {} for a package download",
            resp.status().as_u16()
        )));
    }
    let mut out = Vec::new();
    let mut s = resp.bytes_stream();
    while let Some(chunk) = s.next().await {
        out.extend_from_slice(
            &chunk.map_err(|e| ToolError::Network(chummy_http::reached(host.clone(), &e)))?,
        );
        if out.len() as u64 > MAX_WHEEL {
            return Err(ToolError::Dependencies("a package is too large".into()));
        }
    }
    Ok(out)
}

/// Unpacks a pure wheel. `<pkg>.data/purelib` and `platlib` go to the top
/// level; scripts, headers and data files aren't needed and are skipped.
/// Compiled files are refused even if a wheel is mislabelled.
pub(crate) fn unpack_wheel(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(format!("unsafe path in archive: {:?}", entry.name().ok()));
        };
        let rel = match data_target(&rel) {
            DataTarget::Keep(p) => p,
            DataTarget::Skip => continue,
        };
        let ext = rel
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if matches!(ext.as_str(), "so" | "pyd" | "dll" | "dylib" | "wasm") {
            return Err(format!("it contains compiled code ({})", rel.display()));
        }
        let out = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut data = Vec::with_capacity(entry.size() as usize);
        entry
            .by_ref()
            .take(MAX_WHEEL)
            .read_to_end(&mut data)
            .map_err(|e| e.to_string())?;
        std::fs::write(&out, data).map_err(|e| e.to_string())?;
    }
    Ok(())
}

enum DataTarget {
    Keep(PathBuf),
    Skip,
}

fn data_target(rel: &Path) -> DataTarget {
    let mut comps = rel.components();
    let Some(first) = comps.next() else {
        return DataTarget::Skip;
    };
    let first = first.as_os_str().to_string_lossy();
    if !first.ends_with(".data") {
        return DataTarget::Keep(rel.to_path_buf());
    }
    match comps
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
    {
        Some(k) if k == "purelib" || k == "platlib" => {
            DataTarget::Keep(comps.as_path().to_path_buf())
        }
        _ => DataTarget::Skip,
    }
}

/// Where `uv` keeps its files for tool resolution.
#[derive(Debug, Clone)]
pub struct UvSetup {
    pub uv: PathBuf,
    /// uv's download cache.
    pub cache_dir: PathBuf,
    /// Where uv may install a Python it needs for resolving. (uv needs an
    /// interpreter to resolve; this one never runs tool code.)
    pub python_dir: PathBuf,
}

/// Resolves `dependencies` to a lock of pure-Python wheels with `uv`.
pub async fn resolve(
    uv: &UvSetup,
    dependencies: &[String],
    work: &Path,
) -> Result<String, ToolError> {
    std::fs::create_dir_all(work).map_err(ToolError::io("preparing to resolve dependencies"))?;
    let input = work.join("requirements.in");
    let output = work.join("pylock.toml");
    std::fs::write(&input, dependencies.join("\n") + "\n")
        .map_err(ToolError::io("preparing to resolve dependencies"))?;
    let minor: String = PYTHON_VERSION
        .split('.')
        .take(2)
        .collect::<Vec<_>>()
        .join(".");
    let out = tokio::process::Command::new(&uv.uv)
        .args(["pip", "compile", "--no-config", "--quiet", "--no-header"])
        // The closest platform uv knows: WebAssembly, so only wheels without
        // compiled code (or WebAssembly builds, refused later) qualify.
        .args(["--python-platform", "wasm32-pyodide2025"])
        .args(["--python-version", &minor, "--only-binary", ":all:"])
        .arg("-o")
        .arg(&output)
        .arg(&input)
        .env("UV_CACHE_DIR", &uv.cache_dir)
        .env("UV_PYTHON_INSTALL_DIR", &uv.python_dir)
        .env("UV_PYTHON_DOWNLOADS", "automatic")
        .env("UV_NO_PROGRESS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(ToolError::io("starting uv"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err
            .lines()
            .map(str::trim)
            .find(|l| {
                l.contains("unsatisfiable")
                    || l.starts_with("error")
                    || l.contains("no usable wheels")
            })
            .or_else(|| err.lines().map(str::trim).find(|l| !l.is_empty()))
            .unwrap_or("uv failed")
            .trim_start_matches("error: ")
            .trim_start_matches("╰─▶ ");
        return Err(ToolError::Dependencies(format!(
            "the dependencies can't be resolved to pure-Python packages: {line}"
        )));
    }
    std::fs::read_to_string(&output).map_err(ToolError::io("reading the lock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn wheel_tags_and_purity() {
        assert!(is_pure_wheel("tzdata-2025.2-py2.py3-none-any.whl"));
        assert!(is_pure_wheel("bs4-4.13.4-py3-none-any.whl"));
        assert!(!is_pure_wheel(
            "PyYAML-6.0-cp312-cp312-manylinux_2_17_x86_64.whl"
        ));
        assert!(!is_pure_wheel(
            "numpy-2.0-cp313-cp313-pyodide_2025_0_wasm32.whl"
        ));
        assert!(!is_pure_wheel("old-1.0-py2-none-any.whl"));
        assert!(!is_pure_wheel("pkg-1.0.tar.gz"));
    }

    #[test]
    fn plan_needs_pure_hashed_https_wheels() {
        let ok = PyLock::parse(include_str!("../builtin/pylock.fetch_page.toml")).unwrap();
        assert_eq!(plan(&ok).unwrap().len(), ok.packages.len());

        let no_hash = r#"lock-version = "1.0"
[[packages]]
name = "x"
wheels = [{ url = "https://h.test/x-1-py3-none-any.whl" }]"#;
        assert!(plan(&PyLock::parse(no_hash).unwrap()).is_err());
        let compiled = r#"lock-version = "1.0"
[[packages]]
name = "x"
wheels = [{ url = "https://h.test/x-1-cp314-cp314-win_amd64.whl", hashes = { sha256 = "ab" } }]"#;
        assert!(plan(&PyLock::parse(compiled).unwrap()).is_err());
        assert!(PyLock::parse("lock-version = \"2.0\"").is_err());
        let marker = r#"lock-version = "1.0"
[[packages]]
name = "colorama"
marker = "sys_platform == 'win32'"
wheels = []"#;
        assert!(PyLock::parse(marker).is_err());
        assert!(PyLock::parse("lock-version = \"1.0\"\nrequires-python = \">=3.15\"").is_err());
    }

    fn wheel(files: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            for (name, body) in files {
                z.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn unpacks_safely() {
        let dir = tempfile::tempdir().unwrap();
        let w = wheel(&[
            ("pkg/__init__.py", "x = 1"),
            ("pkg-1.0.dist-info/WHEEL", "Tag: py3-none-any"),
            ("pkg-1.0.data/purelib/extra.py", "y = 2"),
            ("pkg-1.0.data/scripts/run", "#!/bin/sh"),
        ]);
        unpack_wheel(&w, dir.path()).unwrap();
        assert!(dir.path().join("pkg/__init__.py").exists());
        assert!(dir.path().join("extra.py").exists());
        assert!(!dir.path().join("run").exists());

        let evil = wheel(&[("../escape.py", "boom")]);
        assert!(unpack_wheel(&evil, dir.path()).is_err());
        assert!(!dir.path().parent().unwrap().join("escape.py").exists());
        let compiled = wheel(&[("pkg/_speed.so", "ELF")]);
        assert!(unpack_wheel(&compiled, dir.path()).is_err());
    }
}
