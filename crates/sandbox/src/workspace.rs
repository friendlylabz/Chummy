//! Deciding whether a folder may become a workspace.

use std::path::{Path, PathBuf};

use crate::paths::{depth, is_within};
use crate::protected::Protected;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceError {
    #[error("that folder doesn't exist or can't be opened")]
    NotFound,
    #[error("that isn't a folder")]
    NotADirectory,
    #[error("a whole drive can't be a workspace; pick a folder on it instead")]
    DriveRoot,
    #[error("your home folder can't be a workspace; pick a folder inside it instead")]
    HomeFolder,
    #[error("hidden settings folders in your home folder can't be workspaces")]
    HiddenInHome,
    #[error("that folder is inside {0}, which agents may never see")]
    InsideProtected(String),
    #[error("that folder contains {0}, which agents may never see")]
    ContainsProtected(String),
}

/// Checks that `candidate` may be used as a workspace and returns its
/// canonical path. Every rule here is absolute: there is no override.
pub fn validate_workspace(
    candidate: &Path,
    protected: &Protected,
) -> Result<PathBuf, WorkspaceError> {
    let canon = std::fs::canonicalize(candidate).map_err(|_| WorkspaceError::NotFound)?;
    let meta = std::fs::metadata(&canon).map_err(|_| WorkspaceError::NotFound)?;
    if !meta.is_dir() {
        return Err(WorkspaceError::NotADirectory);
    }

    if depth(&canon) == 0 || is_mount_point(&canon) {
        return Err(WorkspaceError::DriveRoot);
    }

    if let Some(home) = &protected.home {
        if is_within(home, &canon) && depth(&canon) == depth(home) {
            return Err(WorkspaceError::HomeFolder);
        }
        if is_within(&canon, home)
            && let Ok(rest) = canon.strip_prefix(home)
            && rest
                .components()
                .next()
                .is_some_and(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        {
            return Err(WorkspaceError::HiddenInHome);
        }
    }

    if let Some((p, inside)) = protected.violation(&canon) {
        if protected.home.as_deref() == Some(p.path.as_path()) {
            return Err(WorkspaceError::HomeFolder);
        }
        return Err(if inside {
            WorkspaceError::InsideProtected(p.label.clone())
        } else {
            WorkspaceError::ContainsProtected(p.label.clone())
        });
    }

    Ok(canon)
}

/// A folder on a different device from its parent is the root of a mounted
/// drive (USB stick, second disk, network share).
#[cfg(unix)]
fn is_mount_point(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(parent) = path.parent() else {
        return true;
    };
    match (std::fs::metadata(path), std::fs::metadata(parent)) {
        (Ok(a), Ok(b)) => a.dev() != b.dev(),
        _ => true,
    }
}

#[cfg(not(unix))]
fn is_mount_point(_path: &Path) -> bool {
    // On Windows, drive and share roots have no name components and are
    // caught by the depth check.
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protected::Scope;

    #[test]
    fn plain_folder_is_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("project");
        std::fs::create_dir(&ws).unwrap();
        let got = validate_workspace(&ws, &Protected::none()).unwrap();
        assert_eq!(got, std::fs::canonicalize(&ws).unwrap());
    }

    #[test]
    fn missing_and_file_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(
            validate_workspace(&tmp.path().join("nope"), &Protected::none()),
            Err(WorkspaceError::NotFound)
        );
        assert_eq!(
            validate_workspace(&file, &Protected::none()),
            Err(WorkspaceError::NotADirectory)
        );
    }

    #[test]
    fn chummy_folder_inside_and_around_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let chummy = tmp.path().join("apps").join("chummy");
        let inside = chummy.join("loops");
        std::fs::create_dir_all(&inside).unwrap();
        let mut p = Protected::none();
        p.add(&chummy, "Chummy's own folder", Scope::Subtree);

        assert!(matches!(
            validate_workspace(&chummy, &p),
            Err(WorkspaceError::ContainsProtected(_))
        ));
        assert!(matches!(
            validate_workspace(&inside, &p),
            Err(WorkspaceError::InsideProtected(_))
        ));
        assert!(matches!(
            validate_workspace(&tmp.path().join("apps"), &p),
            Err(WorkspaceError::ContainsProtected(_))
        ));
        // A sibling is fine.
        let sibling = tmp.path().join("work");
        std::fs::create_dir(&sibling).unwrap();
        assert!(validate_workspace(&sibling, &p).is_ok());
    }

    #[test]
    fn symlink_to_protected_folder_is_rejected() {
        #[cfg(unix)]
        {
            let tmp = tempfile::tempdir().unwrap();
            let chummy = tmp.path().join("chummy");
            std::fs::create_dir(&chummy).unwrap();
            let link = tmp.path().join("innocent");
            std::os::unix::fs::symlink(&chummy, &link).unwrap();
            let mut p = Protected::none();
            p.add(&chummy, "Chummy's own folder", Scope::Subtree);
            assert!(validate_workspace(&link, &p).is_err());
        }
    }

    #[test]
    fn exact_only_allows_subfolders() {
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        let usb = media.join("usb");
        std::fs::create_dir_all(&usb).unwrap();
        let mut p = Protected::none();
        p.add(&media, "a shared top-level folder", Scope::ExactOnly);
        assert!(validate_workspace(&media, &p).is_err());
        assert!(validate_workspace(&usb, &p).is_ok());
    }

    #[test]
    fn home_inside_a_system_folder_still_works() {
        // e.g. root's home in /root, or /var/home on Fedora Silverblue.
        let tmp = tempfile::tempdir().unwrap();
        let var = tmp.path().join("var");
        let home = var.join("home").join("me");
        let proj = home.join("projects");
        let lib = var.join("lib");
        for d in [&proj, &lib, &home.join(".ssh")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let mut p = Protected::none();
        p.home = Some(std::fs::canonicalize(&home).unwrap());
        p.add(&var, "a system folder", Scope::Subtree);
        p.add(&home, "your home folder", Scope::ExactOnly);
        assert!(validate_workspace(&proj, &p).is_ok());
        assert!(validate_workspace(&lib, &p).is_err());
        assert_eq!(
            validate_workspace(&home, &p),
            Err(WorkspaceError::HomeFolder)
        );
        assert_eq!(
            validate_workspace(&home.join(".ssh"), &p),
            Err(WorkspaceError::HiddenInHome)
        );
    }

    #[test]
    fn real_machine_rules() {
        let p = Protected::for_current_user::<&Path>(&[]);
        let root = if cfg!(windows) {
            PathBuf::from("C:\\")
        } else {
            PathBuf::from("/")
        };
        assert_eq!(
            validate_workspace(&root, &p),
            Err(WorkspaceError::DriveRoot)
        );

        if let Some(home) = dirs::home_dir() {
            assert_eq!(
                validate_workspace(&home, &p),
                Err(WorkspaceError::HomeFolder)
            );
            if let Some(parent) = home.parent() {
                assert!(validate_workspace(parent, &p).is_err());
            }
        }
        if let Some(cfg) = dirs::config_dir()
            && cfg.exists()
        {
            assert!(validate_workspace(&cfg, &p).is_err());
        }
        #[cfg(unix)]
        {
            assert!(validate_workspace(Path::new("/etc"), &p).is_err());
            assert!(validate_workspace(Path::new("/usr/bin"), &p).is_err());
        }
        #[cfg(windows)]
        {
            if let Some(win) = std::env::var_os("SystemRoot") {
                assert!(validate_workspace(Path::new(&win), &p).is_err());
            }
        }
    }
}

/// Warnings about a folder that may be used but probably shouldn't be: a
/// whole personal folder (Documents, Desktop, a synced cloud folder) or one
/// that already holds many files. Shown when the user confirms the folder,
/// so nobody hands an agent their whole Documents folder by accident.
pub fn workspace_advice(path: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let canon = |p: PathBuf| std::fs::canonicalize(&p).unwrap_or(p);
    let named = [
        (dirs::document_dir(), "your whole Documents folder"),
        (dirs::desktop_dir(), "your whole Desktop"),
        (dirs::download_dir(), "your whole Downloads folder"),
        (dirs::picture_dir(), "your whole Pictures folder"),
        (dirs::audio_dir(), "your whole Music folder"),
        (dirs::video_dir(), "your whole Videos folder"),
    ];
    for (dir, what) in named {
        if dir.map(canon).is_some_and(|d| d == path) {
            out.push(format!("This is {what}."));
        }
    }
    if let Some(home) = dirs::home_dir().map(canon)
        && path.parent() == Some(home.as_path())
        && let Some(name) = path.file_name().and_then(|n| n.to_str())
    {
        let lower = name.to_lowercase();
        if ["onedrive", "dropbox", "icloud", "google drive", "box"]
            .iter()
            .any(|c| lower.starts_with(c))
        {
            out.push(format!(
                "This is your whole {name} folder, which syncs to other devices."
            ));
        }
    }
    if path.join(".git").exists() {
        out.push(
            "It's a git repository; the agent could change files that are under version control."
                .into(),
        );
    }
    let files = count_files(path, 2, ADVICE_FILE_LIMIT);
    if files >= ADVICE_FILE_LIMIT {
        out.push(format!("It already holds over {ADVICE_FILE_LIMIT} files."));
    }
    out
}

const ADVICE_FILE_LIMIT: usize = 500;

/// Counts files up to `depth` levels down, stopping at `limit`.
fn count_files(dir: &Path, depth: usize, limit: usize) -> usize {
    let mut n = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for e in entries.flatten() {
        if n >= limit {
            break;
        }
        match e.file_type() {
            Ok(t) if t.is_dir() && depth > 0 => n += count_files(&e.path(), depth - 1, limit - n),
            Ok(t) if t.is_file() => n += 1,
            _ => {}
        }
    }
    n.min(limit)
}

#[cfg(test)]
mod advice_tests {
    use super::*;

    #[test]
    fn busy_folders_and_repositories_get_advice() {
        let tmp = tempfile::tempdir().unwrap();
        let empty = tmp.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(workspace_advice(&empty).is_empty());

        let busy = tmp.path().join("busy");
        std::fs::create_dir_all(busy.join("a/b")).unwrap();
        for i in 0..ADVICE_FILE_LIMIT {
            std::fs::write(busy.join("a/b").join(format!("{i}.txt")), "").unwrap();
        }
        std::fs::create_dir(busy.join(".git")).unwrap();
        let advice = workspace_advice(&busy).join(" ");
        assert!(
            advice.contains("over 500 files") && advice.contains("git repository"),
            "{advice}"
        );
    }
}
