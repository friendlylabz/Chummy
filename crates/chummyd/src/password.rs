//! The bundle's key: from a password file (a Docker secret, say), which
//! holds the password's hash ([`BundleKey::to_line`]), never the password;
//! or from the password, asked for in the terminal without showing it.
//! Never from the command line. The password is wiped from memory once the
//! key is derived.

use std::io::IsTerminal;
use std::path::Path;

use chummy_bundle::BundleKey;
use zeroize::Zeroizing;

/// Largest password file read.
const MAX_FILE: u64 = 4096;

/// The key for `bundle`: from `file`, or from the password asked for.
pub fn key(bundle: &[u8], file: Option<&Path>) -> Result<BundleKey, String> {
    match file {
        Some(path) => from_file(path),
        None => {
            let pw = ask()?;
            BundleKey::for_bundle(bundle, &pw).map_err(|e| e.to_string())
        }
    }
}

/// The password: asked for in the terminal, or one line on standard input
/// when that isn't a terminal (`chummyd hash-password`, piped).
pub fn ask() -> Result<Zeroizing<String>, String> {
    if std::io::stdin().is_terminal() {
        return rpassword::prompt_password("Bundle password: ")
            .map(Zeroizing::new)
            .map_err(|e| format!("couldn't ask for the password ({e})"));
    }
    let mut line = Zeroizing::new(String::new());
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("couldn't read the password: {e}"))?;
    let pw = one_line(&line);
    if pw.is_empty() {
        return Err("no password given (on standard input)".into());
    }
    Ok(Zeroizing::new(pw.to_string()))
}

/// Without one line ending (as editors and `echo` add).
fn one_line(raw: &str) -> &str {
    raw.strip_suffix("\r\n")
        .or_else(|| raw.strip_suffix('\n'))
        .unwrap_or(raw)
}

fn from_file(path: &Path) -> Result<BundleKey, String> {
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
                    "The password file {} can be read by other users; make it private (chmod 400)",
                    path.display()
                ),
            );
        }
    }
    let raw = Zeroizing::new(
        std::fs::read_to_string(path)
            .map_err(|e| format!("can't read the password file {}: {e}", path.display()))?,
    );
    BundleKey::from_line(one_line(&raw)).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_file_holds_the_hash_not_the_password() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("pw");
        let k = BundleKey::from_password("secret words here", "i-0123456789abcdef").unwrap();
        std::fs::write(&p, format!("{}\n", *k.to_line())).unwrap();
        assert_eq!(from_file(&p).unwrap(), k);
        std::fs::write(&p, format!("{}\r\n", *k.to_line())).unwrap();
        assert_eq!(from_file(&p).unwrap(), k);
        // The password itself is refused, saying how to make the file.
        std::fs::write(&p, "secret words here\n").unwrap();
        let e = from_file(&p).unwrap_err();
        assert!(
            e.contains("never the password") && e.contains("chummyd-docker-init"),
            "{e}"
        );
        std::fs::write(&p, "\n").unwrap();
        assert!(from_file(&p).is_err());
        assert!(from_file(&tmp.path().join("missing")).is_err());
        assert!(from_file(tmp.path()).is_err(), "a folder isn't one");
        assert_eq!(one_line("a b\n"), "a b");
    }
}
