//! Path helpers: relative-path parsing and case-aware containment checks.

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Whether the platform's default filesystems ignore case (macOS, Windows).
const CASE_INSENSITIVE: bool = cfg!(any(target_os = "macos", windows));

fn same_component(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    if CASE_INSENSITIVE {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// True if `child` is `parent` or lies beneath it. Both should be canonical.
pub(crate) fn is_within(child: &Path, parent: &Path) -> bool {
    let mut c = child.components();
    for p in parent.components() {
        match c.next() {
            Some(cc) if same_component(cc.as_os_str(), p.as_os_str()) => {}
            _ => return false,
        }
    }
    true
}

/// Number of ordinary name components (0 for `/`, `C:\` or `\\server\share\`).
pub(crate) fn depth(path: &Path) -> usize {
    path.components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .count()
}

/// A path an agent gave, checked to be strictly relative to the workspace.
///
/// Rules: no absolute paths, no drive or UNC prefixes, no `..`, no NUL bytes,
/// and on Windows no `:` (alternate data streams) in names. `.` components and
/// repeated separators are dropped. The empty path means the workspace root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelPath(PathBuf);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RelPathError {
    #[error("use a path inside the workspace, not an absolute path: {0}")]
    Absolute(String),
    #[error("'..' is not allowed in paths: {0}")]
    ParentDir(String),
    #[error("path contains a character that isn't allowed: {0}")]
    BadChar(String),
}

impl RelPath {
    pub fn parse(input: &str) -> Result<Self, RelPathError> {
        if input.contains('\0') {
            return Err(RelPathError::BadChar(input.escape_debug().to_string()));
        }
        // Treat both separators the same everywhere so "a\..\..\x" can't
        // slip through on Unix and later be used on Windows.
        let unified = input.replace('\\', "/");
        if unified.starts_with('/') || looks_like_drive(&unified) {
            return Err(RelPathError::Absolute(input.into()));
        }

        let mut out = PathBuf::new();
        for part in unified.split('/') {
            match part {
                "" | "." => {}
                ".." => return Err(RelPathError::ParentDir(input.into())),
                name => {
                    if cfg!(windows) && !windows_safe_name(name) {
                        return Err(RelPathError::BadChar(input.into()));
                    }
                    out.push(name);
                }
            }
        }
        // Belt and braces: whatever we built must still be plain names only.
        if !out.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err(RelPathError::Absolute(input.into()));
        }
        Ok(Self(out))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// True for the workspace root itself.
    pub fn is_root(&self) -> bool {
        self.0.as_os_str().is_empty()
    }

    /// Path to hand to `cap-std` (which wants "." for the root).
    pub(crate) fn for_cap(&self) -> &Path {
        if self.is_root() {
            Path::new(".")
        } else {
            &self.0
        }
    }
}

/// Names Windows handles specially: `:` (alternate data streams), device
/// names like `CON` or `com1.txt`, and trailing dots or spaces (which Windows
/// drops, so the name would mean a different file).
pub(crate) fn windows_safe_name(name: &str) -> bool {
    if name.contains(':') || name.ends_with('.') || name.ends_with(' ') {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    let device = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ((stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.len() == 4
        && stem.as_bytes()[3].is_ascii_digit());
    !device
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.0.to_string_lossy().replace('\\', "/");
        f.write_str(if s.is_empty() { "." } else { &s })
    }
}

fn looks_like_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_relative_paths() {
        assert_eq!(
            RelPath::parse("a/b.txt").unwrap().as_path(),
            Path::new("a").join("b.txt")
        );
        assert_eq!(
            RelPath::parse("./a//b/").unwrap().as_path(),
            Path::new("a").join("b")
        );
        assert!(RelPath::parse("").unwrap().is_root());
        assert!(RelPath::parse(".").unwrap().is_root());
        assert_eq!(
            RelPath::parse("notes/today.md").unwrap().to_string(),
            "notes/today.md"
        );

        // (was windows_names)
        {
            for bad in [
                "CON",
                "nul",
                "nul.txt",
                "COM1",
                "lpt9.log",
                "a:b",
                "trailing.",
                "space ",
            ] {
                assert!(!windows_safe_name(bad), "{bad}");
            }
            for ok in ["console", "com10", "notes.txt", "nul-ish", "COM", ".hidden"] {
                assert!(windows_safe_name(ok), "{ok}");
            }
        }
    }

    #[test]
    fn rejects_escapes() {
        for bad in [
            "/etc/passwd",
            "\\Windows\\System32",
            "C:\\Windows",
            "c:relative",
            "..",
            "../secret",
            "a/../../b",
            "a\\..\\..\\b",
            "a/..",
            "//server/share",
            "\\\\?\\C:\\x",
            "nul\0byte",
        ] {
            assert!(RelPath::parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn containment_is_component_wise() {
        assert!(is_within(Path::new("/a/b/c"), Path::new("/a/b")));
        assert!(is_within(Path::new("/a/b"), Path::new("/a/b")));
        assert!(!is_within(Path::new("/a/bc"), Path::new("/a/b")));
        assert!(!is_within(Path::new("/a"), Path::new("/a/b")));

        // (was depth_counts_names)
        {
            assert_eq!(depth(Path::new("/")), 0);
            assert_eq!(depth(Path::new("/home/me")), 2);
        }
    }
}
