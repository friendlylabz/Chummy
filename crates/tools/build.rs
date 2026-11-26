//! With the `bundled-python` feature, puts the WebAssembly Python that tools
//! run on (built by `scripts/build-wasi-python.sh`) where `bundled.rs`
//! embeds it. Looked for at `$CHUMMY_WASI_PYTHON`, else
//! `wasi-python/python-wasi.zip` in the repository. Without it the crate
//! still builds, and setting up tools says how to get it, unless
//! `CHUMMY_REQUIRE_BUNDLED_PYTHON` is set (release builds).

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_BUNDLED_PYTHON");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("python-wasi.zip");
    if std::env::var_os("CARGO_FEATURE_BUNDLED_PYTHON").is_none() {
        std::fs::write(&out, b"").expect("writing a placeholder");
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let zip = std::env::var_os("CHUMMY_WASI_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../../wasi-python/python-wasi.zip"));
    println!("cargo:rerun-if-env-changed=CHUMMY_WASI_PYTHON");
    println!("cargo:rerun-if-env-changed=CHUMMY_REQUIRE_BUNDLED_PYTHON");
    println!("cargo:rerun-if-changed={}", zip.display());
    if zip.is_file() {
        std::fs::copy(&zip, &out).expect("copying the WebAssembly Python");
    } else if std::env::var_os("CHUMMY_REQUIRE_BUNDLED_PYTHON").is_some() {
        panic!(
            "no WebAssembly Python at {}: run scripts/build-wasi-python.sh",
            zip.display()
        );
    } else {
        println!(
            "cargo:warning=building without the tools' Python ({} not found; see scripts/build-wasi-python.sh)",
            zip.display()
        );
        std::fs::write(&out, b"").expect("writing a placeholder");
    }
}
