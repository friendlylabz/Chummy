//! Versioned JSON documents with crash-safe writes.
//!
//! - **Atomic writes.** A save writes `<file>.tmp`, flushes it to disk, copies
//!   the previous version to `<file>.bak`, then renames the temp file over the
//!   original. A crash at any point leaves either the old or the new file.
//! - **Recovery.** If the file is missing but a backup exists, the backup is
//!   used. If the file is unreadable, it is set aside as
//!   `<file>.corrupt-<unix time>` (never deleted) and the backup is tried, then
//!   defaults.
//! - **Migrations.** Documents carry a `version` field. [`JsonDoc::load`] runs a
//!   caller-supplied migration on the raw JSON before deserializing, so old
//!   files keep working after schema changes.

use std::fs::{self, File};
use std::io::{self, Write};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::layout::restrict_to_owner;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("could not access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not encode the file: {0}")]
    Encode(#[from] serde_json::Error),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> StoreError + '_ {
    move |source| StoreError::Io {
        path: path.to_owned(),
        source,
    }
}

/// What happened while loading, if anything unusual.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Recovery {
    /// The main file was missing or unreadable; the backup was used.
    UsedBackup { reason: String },
    /// Nothing usable was found; defaults were used.
    UsedDefaults { reason: String },
}

/// A loaded document plus any recovery that was needed.
#[derive(Debug)]
pub struct Loaded<T> {
    pub value: T,
    /// `None` for a normal load (including first launch, when no file exists).
    pub recovery: Option<Recovery>,
    /// Where an unreadable file was moved to, if any.
    pub quarantined: Option<PathBuf>,
}

/// A JSON document on disk.
pub struct JsonDoc<T> {
    path: PathBuf,
    _t: PhantomData<fn() -> T>,
}

impl<T: Serialize + DeserializeOwned + Default> JsonDoc<T> {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            _t: PhantomData,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn sibling(&self, suffix: &str) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(suffix);
        self.path.with_file_name(name)
    }

    /// Loads the document, applying `migrate` to the raw JSON first.
    pub fn load(
        &self,
        migrate: impl Fn(serde_json::Value) -> serde_json::Value,
    ) -> Result<Loaded<T>, StoreError> {
        let backup = self.sibling(".bak");

        let main = read_doc::<T>(&self.path, &migrate);
        let mut quarantined = None;
        let reason = match main {
            Ok(Some(value)) => {
                return Ok(Loaded {
                    value,
                    recovery: None,
                    quarantined,
                });
            }
            // First launch: no file and no backup is normal, not a recovery.
            Ok(None) if !backup.exists() => {
                return Ok(Loaded {
                    value: T::default(),
                    recovery: None,
                    quarantined,
                });
            }
            Ok(None) => "the file was missing".to_string(),
            Err(e) => {
                let dest = self.sibling(&format!(".corrupt-{}", unix_now()));
                fs::rename(&self.path, &dest).map_err(io_err(&self.path))?;
                quarantined = Some(dest);
                format!("the file was unreadable ({e})")
            }
        };

        match read_doc::<T>(&backup, &migrate) {
            Ok(Some(value)) => Ok(Loaded {
                value,
                recovery: Some(Recovery::UsedBackup { reason }),
                quarantined,
            }),
            _ => Ok(Loaded {
                value: T::default(),
                recovery: Some(Recovery::UsedDefaults { reason }),
                quarantined,
            }),
        }
    }

    /// Saves the document atomically, keeping the previous version as a backup.
    pub fn save(&self, value: &T) -> Result<(), StoreError> {
        let mut json = serde_json::to_vec_pretty(value)?;
        json.push(b'\n');
        write_atomic(&self.path, &json, true)
    }
}

/// Writes `bytes` to `path` so that a crash leaves either the old or the new
/// file: a flushed temp file renamed over the original. With `backup`, the
/// previous version is copied to `<file>.bak` first. The file is private to
/// the user on Unix.
pub fn write_atomic(path: &Path, bytes: &[u8], backup: bool) -> Result<(), StoreError> {
    let sibling = |suffix: &str| {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(suffix);
        path.with_file_name(name)
    };
    let tmp = sibling(".tmp");
    {
        let mut f = File::create(&tmp).map_err(io_err(&tmp))?;
        f.write_all(bytes).map_err(io_err(&tmp))?;
        f.sync_all().map_err(io_err(&tmp))?;
    }
    restrict_to_owner(&tmp).map_err(io_err(&tmp))?;
    if backup && path.exists() {
        let b = sibling(".bak");
        fs::copy(path, &b).map_err(io_err(&b))?;
    }
    // On Windows, std's rename replaces an existing file.
    fs::rename(&tmp, path).map_err(io_err(path))?;
    sync_parent(path);
    Ok(())
}

/// `Ok(None)` if the file doesn't exist.
fn read_doc<T: DeserializeOwned>(
    path: &Path,
    migrate: &impl Fn(serde_json::Value) -> serde_json::Value,
) -> Result<Option<T>, String> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let raw: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    serde_json::from_value(migrate(raw))
        .map(Some)
        .map_err(|e| e.to_string())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Flushes the directory entry so the rename survives a power cut (Unix only).
fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent()
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct Doc {
        version: u32,
        name: String,
    }

    fn no_migration(v: serde_json::Value) -> serde_json::Value {
        v
    }

    #[test]
    fn first_launch_gives_defaults_without_recovery() {
        let tmp = tempfile::tempdir().unwrap();
        let doc = JsonDoc::<Doc>::new(tmp.path().join("s.json"));
        let loaded = doc.load(no_migration).unwrap();
        assert_eq!(loaded.value, Doc::default());
        assert!(loaded.recovery.is_none());
    }

    #[test]
    fn save_then_load_round_trips_and_keeps_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let doc = JsonDoc::<Doc>::new(tmp.path().join("s.json"));
        doc.save(&Doc {
            version: 1,
            name: "a".into(),
        })
        .unwrap();
        doc.save(&Doc {
            version: 1,
            name: "b".into(),
        })
        .unwrap();
        assert_eq!(doc.load(no_migration).unwrap().value.name, "b");
        let bak: Doc =
            serde_json::from_slice(&fs::read(tmp.path().join("s.json.bak")).unwrap()).unwrap();
        assert_eq!(bak.name, "a");
        assert!(!tmp.path().join("s.json.tmp").exists());
    }

    #[test]
    fn corrupt_file_is_quarantined_and_backup_used() {
        let tmp = tempfile::tempdir().unwrap();
        let doc = JsonDoc::<Doc>::new(tmp.path().join("s.json"));
        doc.save(&Doc {
            version: 1,
            name: "good".into(),
        })
        .unwrap();
        doc.save(&Doc {
            version: 1,
            name: "newer".into(),
        })
        .unwrap();
        fs::write(doc.path(), b"{ not json").unwrap();

        let loaded = doc.load(no_migration).unwrap();
        assert_eq!(loaded.value.name, "good");
        assert!(matches!(loaded.recovery, Some(Recovery::UsedBackup { .. })));
        assert!(loaded.quarantined.unwrap().exists());
    }

    #[test]
    fn corrupt_without_backup_falls_back_to_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let doc = JsonDoc::<Doc>::new(tmp.path().join("s.json"));
        fs::write(doc.path(), b"garbage").unwrap();
        let loaded = doc.load(no_migration).unwrap();
        assert_eq!(loaded.value, Doc::default());
        assert!(matches!(
            loaded.recovery,
            Some(Recovery::UsedDefaults { .. })
        ));
    }

    #[test]
    fn migration_runs_before_deserializing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.json");
        fs::write(&path, br#"{ "version": 0, "title": "old field" }"#).unwrap();
        let doc = JsonDoc::<Doc>::new(&path);
        let loaded = doc
            .load(|mut v| {
                if v["version"] == 0 {
                    v["name"] = v["title"].take();
                    v["version"] = 1.into();
                }
                v
            })
            .unwrap();
        assert_eq!(
            loaded.value,
            Doc {
                version: 1,
                name: "old field".into()
            }
        );
    }
}
