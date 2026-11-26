//! What chummyd checks before it runs anything: who it runs as, and where
//! things are. Any failure stops it.
//!
//! The rule that makes the configuration static: the agents' folders (all
//! inside the jail dir) never contain, and are never inside, the bundle,
//! the password file or the state folder, so nothing an agent does can
//! touch them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chummy_sandbox::{Jail, Protected, Scope, validate_workspace};

/// chummyd never runs as root or Administrator.
pub fn not_root() -> Result<(), String> {
    #[cfg(unix)]
    if rustix::process::geteuid().is_root() {
        return Err("chummyd doesn't run as root. Run it as an ordinary user.".into());
    }
    #[cfg(windows)]
    if is_elevated::is_elevated() {
        return Err("chummyd doesn't run as Administrator. Run it as an ordinary user.".into());
    }
    Ok(())
}

/// Where everything is, as real paths (links resolved).
#[derive(Debug, Clone)]
pub struct Paths {
    pub bundle: PathBuf,
    pub password_file: Option<PathBuf>,
    pub state: PathBuf,
    pub jail_dir: PathBuf,
}

/// Makes a private folder (0700 on Unix) if it isn't there.
pub fn make_private_dir(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        return Ok(());
    }
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path)
        .map_err(|e| format!("couldn't make {}: {e}", path.display()))
}

fn real(path: &Path, what: &str) -> Result<PathBuf, String> {
    std::fs::canonicalize(path).map_err(|e| format!("{what} {}: {e}", path.display()))
}

/// Resolves the paths, makes the jail dir and state folder if they're
/// missing, and checks that none of them overlap the jail dir.
pub fn paths(
    bundle: &Path,
    password_file: Option<&Path>,
    state: &Path,
    jail_dir: &Path,
) -> Result<Paths, String> {
    let bundle = real(bundle, "can't find the bundle")?;
    let password_file = password_file
        .map(|p| real(p, "can't find the password file"))
        .transpose()?;
    make_private_dir(state)?;
    let state = real(state, "can't use the state folder")?;
    make_private_dir(jail_dir)?;
    let jail_dir = real(jail_dir, "can't use the jail dir")?;
    let mut others = vec![("the bundle", &bundle), ("the state folder", &state)];
    if let Some(p) = &password_file {
        others.push(("the password file", p));
    }
    for (what, p) in others {
        if p.starts_with(&jail_dir) || jail_dir.starts_with(p) {
            return Err(format!(
                "The jail dir {} and {what} {} overlap. Agents' folders must be apart from chummyd's own files, so nothing an agent does can change them: put them somewhere else.",
                jail_dir.display(),
                p.display()
            ));
        }
    }
    Ok(Paths {
        bundle,
        password_file,
        state,
        jail_dir,
    })
}

/// The places agents may never see: the usual ones for this user, and
/// chummyd's own files and program.
pub fn protected(p: &Paths) -> Protected {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    let mut own: Vec<&Path> = vec![&p.bundle, &p.state];
    if let Some(f) = &p.password_file {
        own.push(f);
    }
    let mut protected = Protected::for_current_user(&own);
    if let Some(d) = exe_dir {
        protected.add(&d, "chummyd's own folder", Scope::Subtree);
    }
    protected
}

/// Makes (if needed) and checks each agent's folder, `<jail dir>/<agent>/`.
pub fn jails<'a>(
    jail_dir: &Path,
    agents: impl IntoIterator<Item = &'a str>,
    protected: &Protected,
) -> Result<BTreeMap<String, PathBuf>, String> {
    validate_workspace(jail_dir, protected).map_err(|e| {
        let hint = if matches!(e, chummy_sandbox::WorkspaceError::DriveRoot) {
            // As a Docker volume is.
            " A mounted volume counts as a drive: use a folder inside it (like /jails/agents)."
        } else {
            ""
        };
        format!(
            "The jail dir {} can't be used: {e}.{hint}",
            jail_dir.display()
        )
    })?;
    let mut out = BTreeMap::new();
    for id in agents {
        let dir = jail_dir.join(id);
        make_private_dir(&dir)?;
        let jail = Jail::open(&dir, protected).map_err(|e| {
            format!(
                "The folder for {id}, {}, can't be used: {e}.",
                dir.display()
            )
        })?;
        out.insert(id.to_string(), jail.root().to_path_buf());
    }
    Ok(out)
}

/// Who chummyd runs as. Tools run inside chummyd (WebAssembly), so they act
/// as this user too, on the agents' folders.
pub fn running_as() -> String {
    #[cfg(unix)]
    {
        format!(
            "user {} (group {})",
            rustix::process::geteuid().as_raw(),
            rustix::process::getegid().as_raw()
        )
    }
    #[cfg(not(unix))]
    {
        "this account".into()
    }
}

/// The most entries looked at in one agent's folder.
#[cfg(unix)]
const MAX_LOOKED_AT: usize = 50_000;

/// Checks that chummyd (and so the agent's tools) can change what's in an
/// agent's folder. The folder itself must be chummyd's: an error says how
/// to fix it otherwise. Files and folders inside that belong to someone
/// else (copied in as root, say) or are read-only come back as a warning.
#[cfg(unix)]
pub fn ownership(id: &str, dir: &Path) -> Result<Option<String>, String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let (uid, gid) = (
        rustix::process::geteuid().as_raw(),
        rustix::process::getegid().as_raw(),
    );
    let fix = |what: &Path| {
        format!(
            "Give it to chummyd's user: chown -R {uid}:{gid} {} (in Docker, see “In Docker” in the chummyd guide).",
            what.display()
        )
    };
    let meta = std::fs::symlink_metadata(dir)
        .map_err(|e| format!("can't look at {}: {e}", dir.display()))?;
    if meta.uid() != uid {
        return Err(format!(
            "{id}'s folder {} belongs to user {}, but chummyd runs as user {uid}, so the agent couldn't write in it. {}",
            dir.display(),
            meta.uid(),
            fix(dir)
        ));
    }
    // chummyd's own: it can open it up to itself.
    if meta.mode() & 0o700 != 0o700 {
        std::fs::set_permissions(
            dir,
            std::fs::Permissions::from_mode((meta.mode() & 0o7777) | 0o700),
        )
        .map_err(|e| format!("can't give chummyd access to {}: {e}", dir.display()))?;
    }

    let (mut others, mut read_only, mut looked) = (0usize, 0usize, 0usize);
    let mut example: Option<String> = None;
    let mut todo = vec![dir.to_path_buf()];
    'walk: while let Some(d) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            looked += 1;
            if looked > MAX_LOOKED_AT {
                break 'walk;
            }
            let Ok(m) = e.metadata() else { continue };
            let path = e.path();
            let rel = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .display()
                .to_string();
            if m.uid() != uid {
                others += 1;
                example.get_or_insert(format!("{rel}, owned by user {}", m.uid()));
            } else if m.mode() & 0o200 == 0 && !m.file_type().is_symlink() {
                read_only += 1;
                example.get_or_insert(format!("{rel}, read-only"));
            }
            // Into folders that are chummyd's own (not through links).
            if m.is_dir() && !m.file_type().is_symlink() {
                todo.push(path);
            }
        }
    }
    if others + read_only == 0 {
        return Ok(None);
    }
    let n = others + read_only;
    let mut why = vec![];
    if others > 0 {
        let verb = if others == 1 { "belongs" } else { "belong" };
        why.push(format!("{others} {verb} to another user"));
    }
    if read_only > 0 {
        let verb = if read_only == 1 { "is" } else { "are" };
        why.push(format!("{read_only} {verb} read-only"));
    }
    Ok(Some(format!(
        "{id} can't change {n} file{} or folder{} in its folder ({}; like {}). {}",
        if n == 1 { "" } else { "s" },
        if n == 1 { "" } else { "s" },
        why.join(", "),
        example.unwrap_or_default(),
        if others > 0 {
            fix(dir)
        } else {
            format!(
                "Make them writable if it should: chmod -R u+w {}",
                dir.display()
            )
        }
    )))
}

#[cfg(not(unix))]
pub fn ownership(_id: &str, _dir: &Path) -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_jail_dir_stays_apart_from_chummyds_files() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let bundle = base.join("home.chummyd");
        std::fs::write(&bundle, b"x").unwrap();
        let jails_dir = base.join("jails");

        let state = base.join("state");

        let p = paths(&bundle, None, &state, &jails_dir).unwrap();
        assert!(p.state.ends_with("state") && p.state.is_dir());
        assert!(p.jail_dir.is_dir());

        // A jail dir holding the bundle, or inside the state folder, is refused.
        assert!(paths(&bundle, None, &state, base).is_err());
        assert!(paths(&bundle, None, &state, &p.state.join("j")).is_err());
        let pw = jails_dir.join("pw");
        std::fs::write(&pw, "x").unwrap();
        assert!(paths(&bundle, Some(&pw), &state, &jails_dir).is_err());
        assert!(paths(&base.join("missing"), None, &state, &jails_dir).is_err());
        // Nor is a state folder inside the jail dir.
        assert!(paths(&bundle, None, &jails_dir.join("s"), &jails_dir).is_err());
    }

    #[test]
    fn each_agent_gets_its_own_folder_and_chummyds_files_are_off_limits() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("srv");
        std::fs::create_dir_all(&base).unwrap();
        // On some systems the temporary folder is itself off-limits (inside
        // Windows' AppData): nothing to check there.
        if validate_workspace(&base, &Protected::for_current_user::<&Path>(&[])).is_err() {
            eprintln!("skipped: the temporary folder can't hold workspaces here");
            return;
        }
        let bundle = base.join("b.chummyd");
        std::fs::write(&bundle, b"x").unwrap();
        let p = paths(&bundle, None, &base.join("state"), &base.join("jails")).unwrap();
        let prot = protected(&p);
        let j = jails(&p.jail_dir, ["helper", "writer"], &prot).unwrap();
        assert_eq!(j.len(), 2);
        assert!(j["helper"].ends_with("jails/helper") && j["helper"].is_dir());
        // The state folder can never be an agent's folder.
        assert!(Jail::open(&p.state, &prot).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn what_an_agent_cant_change_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("helper");
        std::fs::create_dir_all(dir.join("notes")).unwrap();
        std::fs::write(dir.join("notes/a.md"), "x").unwrap();
        assert_eq!(ownership("helper", &dir).unwrap(), None);

        // A read-only file is reported; a folder closed to chummyd is opened.
        std::fs::set_permissions(
            dir.join("notes/a.md"),
            std::fs::Permissions::from_mode(0o444),
        )
        .unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let w = ownership("helper", &dir).unwrap().expect("a warning");
        assert!(
            w.contains("1 file") && w.contains("read-only") && w.contains("notes/a.md"),
            "{w}"
        );
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o700, 0o700);

        // Files of another user (only root can make them).
        if rustix::process::geteuid().is_root() {
            let f = dir.join("root.txt");
            std::fs::write(&f, "x").unwrap();
            rustix::fs::chown(&f, Some(rustix::process::Uid::from_raw(1)), None).unwrap();
            let w = ownership("helper", &dir).unwrap().unwrap();
            assert!(
                w.contains("belongs to another user") && w.contains("chown -R"),
                "{w}"
            );
            rustix::fs::chown(&dir, Some(rustix::process::Uid::from_raw(1)), None).unwrap();
            let e = ownership("helper", &dir).unwrap_err();
            assert!(e.contains("belongs to user 1"), "{e}");
        }
    }
}
