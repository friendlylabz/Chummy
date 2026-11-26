//! Getting the bundle's password: from a file (a Docker secret, say), or
//! asked for in the terminal without showing it. Never from the command
//! line. It's wiped from memory once the bundle is open.

use std::path::Path;

use zeroize::Zeroizing;

/// Largest password file read.
const MAX_FILE: u64 = 4096;

pub fn read(file: Option<&Path>) -> Result<Zeroizing<String>, String> {
    match file {
        Some(path) => from_file(path),
        None => rpassword::prompt_password("Bundle password: ")
            .map(Zeroizing::new)
            .map_err(|e| {
                format!(
                    "couldn't ask for the password ({e}); use --password-file without a terminal"
                )
            }),
    }
}

fn from_file(path: &Path) -> Result<Zeroizing<String>, String> {
    let meta = std::fs::metadata(path)
        .map_err(|e| format!("can't read the password file {}: {e}", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return Err(format!("{} isn't a password file", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            chummy_log::system::warn(
                "security",
                format!(
                    "The password file {} can be read by other users; make it private (chmod 600)",
                    path.display()
                ),
            );
        }
    }
    let raw = Zeroizing::new(
        std::fs::read_to_string(path)
            .map_err(|e| format!("can't read the password file {}: {e}", path.display()))?,
    );
    // One line ending (as editors and `echo` add) isn't part of it.
    let text = raw
        .strip_suffix("\r\n")
        .or_else(|| raw.strip_suffix('\n'))
        .unwrap_or(&raw);
    if text.is_empty() {
        return Err(format!("the password file {} is empty", path.display()));
    }
    Ok(Zeroizing::new(text.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_file_loses_one_line_ending() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("pw");
        std::fs::write(&p, "secret words here\n").unwrap();
        assert_eq!(read(Some(&p)).unwrap().as_str(), "secret words here");
        std::fs::write(&p, "  spaced  \r\n").unwrap();
        assert_eq!(read(Some(&p)).unwrap().as_str(), "  spaced  ");
        std::fs::write(&p, "\n").unwrap();
        assert!(read(Some(&p)).is_err());
        assert!(read(Some(&tmp.path().join("missing"))).is_err());
        assert!(read(Some(tmp.path())).is_err(), "a folder isn't one");
    }
}
