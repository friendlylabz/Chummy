//! File access confined to the workspace.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use serde::Serialize;

use crate::paths::{RelPath, RelPathError};
use crate::protected::Protected;
use crate::workspace::{WorkspaceError, validate_workspace};

#[derive(Debug, thiserror::Error)]
pub enum JailError {
    #[error(transparent)]
    Path(#[from] RelPathError),
    #[error("workspace not allowed: {0}")]
    Workspace(#[from] WorkspaceError),
    #[error("{path}: file is larger than the {limit}-byte limit")]
    TooLarge { path: String, limit: u64 },
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// One entry from [`Jail::list`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    /// A regular file (not a folder, link or device).
    pub is_file: bool,
    pub size: u64,
    /// Last modified, in milliseconds since 1970 (0 if unknown).
    pub modified_ms: i64,
}

/// A handle on a validated workspace. Every operation takes a path relative
/// to the workspace and is resolved by the OS beneath the workspace handle.
pub struct Jail {
    root: PathBuf,
    dir: Dir,
}

impl Jail {
    /// Re-validates `root` against `protected` and opens it.
    pub fn open(root: &Path, protected: &Protected) -> Result<Self, JailError> {
        let root = validate_workspace(root, protected)?;
        let dir =
            Dir::open_ambient_dir(&root, ambient_authority()).map_err(|source| JailError::Io {
                path: root.display().to_string(),
                source,
            })?;
        Ok(Self { root, dir })
    }

    /// The workspace's canonical location on the host. For display only; never
    /// build host paths from it for file access.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The capability handle, for runtimes (such as WASI) that need one.
    pub fn dir(&self) -> &Dir {
        &self.dir
    }

    /// Opens a file without ever blocking: on Unix with `O_NONBLOCK`, so a
    /// named pipe (FIFO) planted in the workspace, for example by a tool in
    /// a container, can't hang Chummy. Then makes sure the opened handle is
    /// a regular file: checking before opening could be raced.
    fn open_file(
        &self,
        rel: &RelPath,
        opts: &mut OpenOptions,
    ) -> Result<cap_std::fs::File, JailError> {
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NONBLOCK);
        }
        let file = self
            .dir
            .open_with(rel.for_cap(), opts)
            .map_err(Self::io(rel))?;
        let meta = file.metadata().map_err(Self::io(rel))?;
        if !meta.is_file() {
            let kind = if meta.is_dir() {
                std::io::ErrorKind::IsADirectory
            } else {
                std::io::ErrorKind::InvalidInput
            };
            return Err(Self::io(rel)(kind.into()));
        }
        Ok(file)
    }

    fn io(path: &RelPath) -> impl FnOnce(std::io::Error) -> JailError + '_ {
        move |source| JailError::Io {
            path: path.to_string(),
            source,
        }
    }

    /// Reads a UTF-8 text file (invalid bytes are replaced), refusing files
    /// larger than `limit` bytes.
    pub fn read_text(&self, path: &str, limit: u64) -> Result<String, JailError> {
        let bytes = self.read_bytes(path, limit)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn read_bytes(&self, path: &str, limit: u64) -> Result<Vec<u8>, JailError> {
        let rel = RelPath::parse(path)?;
        // Only regular files: a named pipe or device would block or never end.
        let file = self.open_file(&rel, OpenOptions::new().read(true))?;
        let len = file.metadata().map_err(Self::io(&rel))?.len();
        if len > limit {
            return Err(JailError::TooLarge {
                path: rel.to_string(),
                limit,
            });
        }
        let mut buf = Vec::with_capacity(len as usize);
        file.take(limit + 1)
            .read_to_end(&mut buf)
            .map_err(Self::io(&rel))?;
        if buf.len() as u64 > limit {
            return Err(JailError::TooLarge {
                path: rel.to_string(),
                limit,
            });
        }
        Ok(buf)
    }

    /// Creates or replaces a file, creating parent folders as needed.
    pub fn write(&self, path: &str, contents: &[u8]) -> Result<(), JailError> {
        let rel = RelPath::parse(path)?;
        if rel.is_root() {
            return Err(Self::io(&rel)(std::io::ErrorKind::IsADirectory.into()));
        }
        if let Some(parent) = rel.as_path().parent()
            && !parent.as_os_str().is_empty()
        {
            self.dir.create_dir_all(parent).map_err(Self::io(&rel))?;
        }
        // Not truncated until it's known to be a regular file.
        let mut f = self.open_file(&rel, OpenOptions::new().write(true).create(true))?;
        f.set_len(0).map_err(Self::io(&rel))?;
        f.write_all(contents).map_err(Self::io(&rel))?;
        Ok(())
    }

    /// Lists a folder, sorted with folders first.
    pub fn list(&self, path: &str) -> Result<Vec<DirEntry>, JailError> {
        let rel = RelPath::parse(path)?;
        let mut out = Vec::new();
        for entry in self.dir.read_dir(rel.for_cap()).map_err(Self::io(&rel))? {
            let entry = entry.map_err(Self::io(&rel))?;
            let meta = entry.metadata().map_err(Self::io(&rel))?;
            let modified_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.into_std().duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            out.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: meta.is_dir(),
                is_file: meta.is_file(),
                size: if meta.is_dir() { 0 } else { meta.len() },
                modified_ms,
            });
        }
        out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
        Ok(out)
    }

    pub fn create_dir(&self, path: &str) -> Result<(), JailError> {
        let rel = RelPath::parse(path)?;
        self.dir
            .create_dir_all(rel.for_cap())
            .map_err(Self::io(&rel))
    }

    pub fn remove_file(&self, path: &str) -> Result<(), JailError> {
        let rel = RelPath::parse(path)?;
        self.dir.remove_file(rel.for_cap()).map_err(Self::io(&rel))
    }

    /// Removes an empty folder. The workspace root itself can't be removed.
    pub fn remove_dir(&self, path: &str) -> Result<(), JailError> {
        let rel = RelPath::parse(path)?;
        if rel.is_root() {
            return Err(Self::io(&rel)(std::io::ErrorKind::PermissionDenied.into()));
        }
        self.dir.remove_dir(rel.as_path()).map_err(Self::io(&rel))
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<(), JailError> {
        let (from, to) = (RelPath::parse(from)?, RelPath::parse(to)?);
        if from.is_root() || to.is_root() {
            return Err(Self::io(&from)(std::io::ErrorKind::PermissionDenied.into()));
        }
        self.dir
            .rename(from.as_path(), &self.dir, to.as_path())
            .map_err(Self::io(&from))
    }

    pub fn exists(&self, path: &str) -> Result<bool, JailError> {
        let rel = RelPath::parse(path)?;
        Ok(self.dir.exists(rel.for_cap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: u64 = 1 << 20;

    /// A workspace next to a folder holding a secret.
    fn setup() -> (tempfile::TempDir, Jail) {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "TOP SECRET").unwrap();
        std::fs::write(ws.join("hello.txt"), "hi").unwrap();
        let jail = Jail::open(&ws, &Protected::none()).unwrap();
        (tmp, jail)
    }

    #[cfg(unix)]
    #[test]
    fn named_pipes_are_not_read() {
        let (_tmp, jail) = setup();
        let fifo = jail.root().join("pipe");
        let ok = std::process::Command::new("mkfifo").arg(&fifo).status();
        if !ok.is_ok_and(|s| s.success()) {
            return; // no mkfifo here
        }
        // Without the check these would block forever.
        assert!(jail.read_text("pipe", LIMIT).is_err());
        assert!(jail.write("pipe", b"x").is_err());
        std::os::unix::fs::symlink("pipe", fifo.with_file_name("to-pipe")).unwrap();
        assert!(jail.read_text("to-pipe", LIMIT).is_err());
        assert!(jail.read_text(".", LIMIT).is_err());
    }

    #[test]
    fn normal_operations_work() {
        let (_tmp, jail) = setup();
        assert_eq!(jail.read_text("hello.txt", LIMIT).unwrap(), "hi");
        jail.write("notes/a/b.md", b"deep").unwrap();
        assert_eq!(jail.read_text("notes/a/b.md", LIMIT).unwrap(), "deep");
        assert_eq!(jail.read_text("./notes//a/b.md", LIMIT).unwrap(), "deep");
        let names: Vec<_> = jail.list("").unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["notes", "hello.txt"]);
        jail.rename("hello.txt", "notes/hello.txt").unwrap();
        assert!(jail.exists("notes/hello.txt").unwrap());
        jail.remove_file("notes/hello.txt").unwrap();
        assert!(!jail.exists("notes/hello.txt").unwrap());
    }

    #[test]
    fn size_limit_is_enforced() {
        let (_tmp, jail) = setup();
        jail.write("big.bin", &[0u8; 100]).unwrap();
        assert!(matches!(
            jail.read_bytes("big.bin", 99),
            Err(JailError::TooLarge { .. })
        ));
        assert!(jail.read_bytes("big.bin", 100).is_ok());
    }

    #[test]
    fn lexical_escapes_are_refused() {
        let (tmp, jail) = setup();
        let abs = tmp.path().join("outside/secret.txt");
        for p in [
            "../outside/secret.txt",
            abs.to_str().unwrap(),
            "a/../../outside/secret.txt",
        ] {
            assert!(jail.read_text(p, LIMIT).is_err(), "read {p}");
            assert!(jail.write(p, b"x").is_err(), "write {p}");
        }
        assert!(jail.remove_dir("").is_err());
        assert!(jail.rename("hello.txt", "../outside/stolen.txt").is_err());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("outside/secret.txt")).unwrap(),
            "TOP SECRET"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_escape() {
        use std::os::unix::fs::symlink;
        let (tmp, jail) = setup();
        let ws = jail.root().to_path_buf();
        symlink(tmp.path().join("outside/secret.txt"), ws.join("link.txt")).unwrap();
        symlink(tmp.path().join("outside"), ws.join("linkdir")).unwrap();
        symlink("/", ws.join("rootlink")).unwrap();

        assert!(jail.read_text("link.txt", LIMIT).is_err());
        assert!(jail.read_text("linkdir/secret.txt", LIMIT).is_err());
        assert!(jail.read_text("rootlink/etc/passwd", LIMIT).is_err());
        assert!(jail.list("linkdir").is_err());
        assert!(jail.write("linkdir/planted.txt", b"x").is_err());
        assert!(jail.write("link.txt", b"overwrite").is_err());
        assert!(!tmp.path().join("outside/planted.txt").exists());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("outside/secret.txt")).unwrap(),
            "TOP SECRET"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_inside_the_workspace_still_work() {
        use std::os::unix::fs::symlink;
        let (_tmp, jail) = setup();
        symlink("hello.txt", jail.root().join("alias.txt")).unwrap();
        assert_eq!(jail.read_text("alias.txt", LIMIT).unwrap(), "hi");
    }

    #[test]
    fn open_revalidates_the_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = Protected::none();
        p.add(tmp.path(), "Chummy's own folder", crate::Scope::Subtree);
        assert!(matches!(
            Jail::open(tmp.path(), &p),
            Err(JailError::Workspace(_))
        ));
    }
}
