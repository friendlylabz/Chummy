//! Installing tools and keeping track of them.
//!
//! An installed tool is a folder under `tools/installed/<name>/`:
//!
//! ```text
//! tool.py          the tool file, unchanged
//! pylock.toml      its resolved dependencies (if any)
//! site-packages/   those dependencies, unpacked
//! manifest.json    what Chummy learned: description, parameters, packages
//! ```
//!
//! Installing goes: read the header (no code runs) → check the Python
//! version → resolve or take the lock → download and check every wheel →
//! load the tool once *inside the sandbox* to read its parameters → write
//! the manifest. The folder is built beside the old one and swapped in at
//! the end, so a failed install leaves the previous version working.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use sha2::{Digest, Sha256};

use crate::ToolError;
use crate::host::{CallSpec, Mode, ToolHost};
use crate::lock::{Package, PyLock, UvSetup, hex, install_lock, resolve};
use crate::meta::{ToolMeta, parse_meta, python_satisfies};
use crate::net::NetPolicy;
use crate::python::{PYTHON_VERSION, ToolPaths};

/// What Chummy knows about an installed tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolManifest {
    pub name: String,
    pub title: String,
    pub version: String,
    /// One sentence for the model, from the function's docstring.
    pub description: String,
    /// JSON Schema for the arguments, from the function's signature.
    pub parameters: Json,
    /// The tool asks for internet access.
    pub network: bool,
    /// Marked "potentially unsafe" (e.g. the command tool).
    #[serde(default)]
    pub risky: bool,
    pub timeout_secs: u32,
    /// What it needs from the sandbox beyond Python (`programs`).
    #[serde(default)]
    pub needs: Vec<String>,
    /// Its `requires-python`, checked against the sandbox's Python.
    #[serde(default)]
    pub requires_python: Option<String>,
    pub dependencies: Vec<String>,
    pub packages: Vec<Package>,
    pub python: String,
    pub source_sha256: String,
    /// Shipped with Chummy.
    pub builtin: bool,
    pub installed_at: u64,
}

/// A tool that ships with Chummy.
pub struct Builtin {
    pub name: &'static str,
    pub source: &'static str,
    pub lock: Option<&'static str>,
}

/// The tools installed with the tool runtime.
pub fn builtins() -> Vec<Builtin> {
    macro_rules! tool {
        ($n:literal) => {
            Builtin {
                name: $n,
                source: include_str!(concat!("../builtin/", $n, ".py")),
                lock: None,
            }
        };
        ($n:literal, lock) => {
            Builtin {
                name: $n,
                source: include_str!(concat!("../builtin/", $n, ".py")),
                lock: Some(include_str!(concat!("../builtin/pylock.", $n, ".toml"))),
            }
        };
    }
    vec![
        tool!("read_file"),
        tool!("write_file"),
        tool!("list_files"),
        tool!("edit_file"),
        tool!("search_files"),
        tool!("find_files"),
        tool!("run_command"),
        tool!("run_shell"),
        tool!("calculator"),
        tool!("current_time", lock),
        tool!("fetch_page", lock),
    ]
}

fn sha(s: &str) -> String {
    hex(&Sha256::digest(s.as_bytes()))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Reads an installed tool's manifest.
pub fn manifest(paths: &ToolPaths, name: &str) -> Option<ToolManifest> {
    if !crate::meta::valid_name(name) {
        return None;
    }
    let dir = paths.tool_dir(name);
    let mut m: ToolManifest =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).ok()?).ok()?;
    if m.name != name {
        return None;
    }
    // Tools installed before sandboxes could be chosen have no `needs` or
    // `requires_python` in their manifest; read them from the header (no
    // code runs).
    if m.needs.is_empty()
        && m.requires_python.is_none()
        && let Ok(src) = std::fs::read_to_string(dir.join("tool.py"))
        && let Ok(meta) = parse_meta(&src)
    {
        m.needs = meta.needs;
        m.requires_python = meta.requires_python;
    }
    Some(m)
}

/// Every installed tool, by name.
pub fn installed(paths: &ToolPaths) -> Vec<ToolManifest> {
    let mut out: Vec<ToolManifest> = std::fs::read_dir(&paths.installed)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| manifest(paths, e.file_name().to_str()?))
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Checks that an installed tool's file is the one that was installed.
pub fn verify(paths: &ToolPaths, m: &ToolManifest) -> Result<(), ToolError> {
    if m.python != PYTHON_VERSION {
        return Err(ToolError::Runtime(format!(
            "{} was installed for Python {}, and tools now run on Python {PYTHON_VERSION}; reinstall it",
            m.name, m.python
        )));
    }
    let src = std::fs::read_to_string(paths.tool_dir(&m.name).join("tool.py"))
        .map_err(|_| ToolError::NotInstalled(m.name.clone()))?;
    if sha(&src) != m.source_sha256 {
        return Err(ToolError::Runtime(format!(
            "{}'s file has changed since it was installed; reinstall it",
            m.name
        )));
    }
    Ok(())
}

/// Removes an installed tool.
pub fn remove(paths: &ToolPaths, name: &str) -> Result<(), ToolError> {
    if !crate::meta::valid_name(name) {
        return Err(ToolError::NotInstalled(name.into()));
    }
    let dir = paths.tool_dir(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(ToolError::io("removing the tool"))?;
    }
    Ok(())
}

/// Installs (or reinstalls) a tool from its source.
///
/// `lock` is a `pylock.toml` for its dependencies; without one, `uv` resolves
/// them (needed only for tools with dependencies).
pub async fn install(
    host: &ToolHost,
    source: &str,
    lock: Option<&str>,
    uv: Option<&UvSetup>,
    builtin: bool,
    on: &mut (dyn FnMut(&str) + Send),
) -> Result<ToolManifest, ToolError> {
    // One install at a time, so two installs of one tool can't swap folders
    // under each other.
    static INSTALLING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one = INSTALLING.lock().await;
    let paths = host.paths().clone();
    // With the lock held, any staging folder is left over from an install
    // that was interrupted.
    for e in std::fs::read_dir(&paths.installed)
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && (name.contains(".installing-") || name.ends_with(".old")) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    let meta: ToolMeta = parse_meta(source)?;
    if let Some(req) = &meta.requires_python
        && !python_satisfies(req, PYTHON_VERSION)?
    {
        return Err(ToolError::Definition(format!(
            "it needs Python {req}, and tools run on Python {PYTHON_VERSION}"
        )));
    }
    if !builtin && manifest(&paths, &meta.name).is_some_and(|m| m.builtin) {
        return Err(ToolError::Definition(format!(
            "“{}” is the name of a built-in tool; give this tool its own name",
            meta.name
        )));
    }
    std::fs::create_dir_all(&paths.installed)
        .map_err(ToolError::io("creating the tools folder"))?;
    let final_dir = paths.tool_dir(&meta.name);
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = paths.installed.join(format!(
        ".{}.installing-{}-{seq}",
        meta.name,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(ToolError::io("preparing the tool folder"))?;
    let result = build(host, &paths, &meta, source, lock, uv, builtin, &tmp, on).await;
    match result {
        Ok(m) => {
            let old = tmp.with_extension("old");
            let _ = std::fs::remove_dir_all(&old);
            if final_dir.exists() {
                rename_retrying(&final_dir, &old)
                    .await
                    .map_err(ToolError::io("replacing the tool"))?;
            }
            if let Err(e) = std::fs::rename(&tmp, &final_dir) {
                let _ = std::fs::rename(&old, &final_dir);
                return Err(ToolError::io("installing the tool")(e));
            }
            let _ = std::fs::remove_dir_all(&old);
            Ok(m)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn build(
    host: &ToolHost,
    paths: &ToolPaths,
    meta: &ToolMeta,
    source: &str,
    lock: Option<&str>,
    uv: Option<&UvSetup>,
    builtin: bool,
    dir: &Path,
    on: &mut (dyn FnMut(&str) + Send),
) -> Result<ToolManifest, ToolError> {
    std::fs::write(dir.join("tool.py"), source).map_err(ToolError::io("writing the tool"))?;
    let mut packages = Vec::new();
    if !meta.dependencies.is_empty() {
        let lock_text = match lock {
            Some(l) => l.to_string(),
            None => {
                let uv = uv.ok_or_else(|| {
                    ToolError::Dependencies(
                        "resolving dependencies needs the package manager".into(),
                    )
                })?;
                on(&format!("Resolving {}'s dependencies", meta.name));
                resolve(
                    uv,
                    &meta.dependencies,
                    &paths.cache.join("resolve").join(&meta.name),
                )
                .await?
            }
        };
        let parsed = PyLock::parse(&lock_text)?;
        std::fs::write(dir.join("pylock.toml"), &lock_text)
            .map_err(ToolError::io("writing the lock"))?;
        packages =
            install_lock(&parsed, &dir.join("site-packages"), &host.http_client(), on).await?;
    }

    on(&format!("Checking {}", meta.name));
    let out = host
        .call(CallSpec {
            tool_dir: dir,
            mode: Mode::Describe,
            workspace: None,
            network: NetPolicy::off("Tools can't use the internet while they're being installed."),
            timeout: Duration::from_secs(60),
            env: vec![],
        })
        .await?;
    let desc = out.result.map_err(|(_, e)| ToolError::Definition(e))?;
    let description = desc
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or_default()
        .to_string();
    let parameters = desc.get("parameters").cloned().unwrap_or(Json::Null);
    if description.is_empty() || !parameters.is_object() {
        return Err(ToolError::Definition(
            "the tool didn't describe itself".into(),
        ));
    }
    let m = ToolManifest {
        name: meta.name.clone(),
        title: meta.title.clone(),
        version: meta.version.clone(),
        description,
        parameters,
        network: meta.network,
        risky: meta.risky,
        timeout_secs: meta.timeout_secs,
        needs: meta.needs.clone(),
        requires_python: meta.requires_python.clone(),
        dependencies: meta.dependencies.clone(),
        packages,
        python: PYTHON_VERSION.into(),
        source_sha256: sha(source),
        builtin,
        installed_at: now(),
    };
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec_pretty(&m).unwrap_or_default(),
    )
    .map_err(ToolError::io("writing the tool manifest"))?;
    Ok(m)
}

/// Installs every built-in tool that's missing or out of date.
pub async fn install_builtins(
    host: &ToolHost,
    on: &mut (dyn FnMut(&str) + Send),
) -> Result<Vec<ToolManifest>, ToolError> {
    let mut out = Vec::new();
    for b in builtins() {
        if let Some(m) = manifest(host.paths(), b.name)
            && m.source_sha256 == sha(b.source)
            && m.python == PYTHON_VERSION
        {
            out.push(m);
            continue;
        }
        on(&format!("Installing {}", b.name));
        out.push(install(host, b.source, b.lock, None, true, on).await?);
    }
    Ok(out)
}

/// Windows refuses to rename a folder while a running tool has files in it
/// open, so a reinstall during a call waits briefly for the call to finish.
async fn rename_retrying(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if cfg!(windows) && tries < 50 && e.kind() != std::io::ErrorKind::NotFound => {
                tries += 1;
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            r => return r,
        }
    }
}
