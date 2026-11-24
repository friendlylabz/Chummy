//! The WebAssembly Python that tools run on, built from python.org's signed
//! source by `scripts/build-wasi-python.sh` and embedded at build time (see
//! `build.rs`). Setting up tools unpacks it; nothing is downloaded.

use std::sync::OnceLock;

static ZIP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/python-wasi.zip"));

/// The zip, or why this copy of Chummy has none (a development build).
pub fn zip() -> Result<&'static [u8], String> {
    if ZIP.is_empty() {
        Err(
            "This copy of Chummy was built without the Python that tools run on. Build it with \
             scripts/build-wasi-python.sh, then build Chummy again."
                .into(),
        )
    } else {
        Ok(ZIP)
    }
}

/// SHA-256 of the bundled zip (what an installed copy is compared with).
pub fn sha256() -> &'static str {
    static SHA: OnceLock<String> = OnceLock::new();
    SHA.get_or_init(|| {
        use sha2::{Digest, Sha256};
        Sha256::digest(ZIP)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    })
}

/// Whether the installed Python is the bundled one.
pub fn is_installed(paths: &chummy_tools::ToolPaths) -> bool {
    chummy_tools::python_status(paths).is_some_and(|i| i.sha256 == sha256())
}

/// Unpacks the bundled Python unless it's already installed. Blocking.
pub fn install(paths: &chummy_tools::ToolPaths) -> Result<(), String> {
    if is_installed(paths) {
        return Ok(());
    }
    chummy_tools::install_python_from(paths, zip()?).map_err(|e| e.to_string())?;
    Ok(())
}

/// Whether tools were set up before (so a new bundled Python can replace the
/// old one without asking: it's part of the app, and nothing is downloaded).
pub fn tools_were_set_up(paths: &chummy_tools::ToolPaths) -> bool {
    std::fs::read_dir(&paths.installed).is_ok_and(|mut d| d.next().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_python_unpacks_once_and_replaces_another() {
        if zip().is_err() {
            eprintln!("skipped: built without the tools' Python");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let paths = chummy_tools::ToolPaths::new(tmp.path());
        assert!(!is_installed(&paths) && !tools_were_set_up(&paths));
        install(&paths).unwrap();
        assert!(is_installed(&paths));
        assert!(paths.wasm().is_file());

        // Already there: not unpacked again.
        let marker = paths.python.join("marker");
        std::fs::write(&marker, "x").unwrap();
        install(&paths).unwrap();
        assert!(marker.exists());

        // A different Python (as after an update) is replaced.
        let ready = paths.python.join("ready.json");
        let mut info: chummy_tools::PythonInfo =
            serde_json::from_slice(&std::fs::read(&ready).unwrap()).unwrap();
        info.sha256 = "0".repeat(64);
        std::fs::write(&ready, serde_json::to_vec(&info).unwrap()).unwrap();
        assert!(!is_installed(&paths));
        install(&paths).unwrap();
        assert!(is_installed(&paths) && !marker.exists());
    }
}
