//! Works out the fingerprint (SHA-256) of every built-in template at every
//! build, so the list of loops Chummy trusts out of the box always matches
//! the templates it ships. The template files are written exactly as Chummy
//! saves a loop (see `templates/make_templates.py`), so the hash of a file
//! is the fingerprint of the loop in it; a test in `templates.rs` checks.

use std::fmt::Write as _;
use std::path::Path;

use sha2::{Digest, Sha256};

fn main() {
    let dir =
        Path::new(&std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo")).join("templates");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("crates/flow/templates exists")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".chummy.json"))
        .collect();
    files.sort();
    let mut out = String::from(
        "/// The SHA-256 of each built-in template file, by file name. Written\n\
         /// by build.rs at every build.\n\
         pub const TEMPLATE_PRINTS: &[(&str, &str)] = &[\n",
    );
    for p in files {
        println!("cargo:rerun-if-changed={}", p.display());
        let text = std::fs::read_to_string(&p).expect("a template file is UTF-8");
        // A checkout may turn line endings into CRLF; Chummy saves LF.
        let text = text.replace("\r\n", "\n");
        let hash: String =
            Sha256::digest(text.as_bytes())
                .iter()
                .fold(String::new(), |mut s, b| {
                    let _ = write!(s, "{b:02x}");
                    s
                });
        let name = p.file_name().and_then(|n| n.to_str()).expect("a file name");
        let _ = writeln!(out, "    ({name:?}, {hash:?}),");
    }
    out.push_str("];\n");
    let dest =
        Path::new(&std::env::var("OUT_DIR").expect("set by cargo")).join("template_prints.rs");
    std::fs::write(dest, out).expect("OUT_DIR is writable");
}
