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

/// The state folder's default place: beside the bundle, named after it.
pub fn default_state(bundle: &Path) -> PathBuf {
    let mut name = bundle.file_name().unwrap_or_default().to_os_string();
    name.push(".state");
    bundle.with_file_name(name)
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
    state: Option<&Path>,
    jail_dir: &Path,
) -> Result<Paths, String> {
    let bundle = real(bundle, "can't find the bundle")?;
    let password_file = password_file
        .map(|p| real(p, "can't find the password file"))
        .transpose()?;
    let state = state
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_state(&bundle));
    make_private_dir(&state)?;
    let state = real(&state, "can't use the state folder")?;
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
    validate_workspace(jail_dir, protected)
        .map_err(|e| format!("The jail dir {} can't be used: {e}.", jail_dir.display()))?;
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

        let p = paths(&bundle, None, None, &jails_dir).unwrap();
        assert!(p.state.ends_with("home.chummyd.state") && p.state.is_dir());
        assert!(p.jail_dir.is_dir());

        // A jail dir holding the bundle, or inside the state folder, is refused.
        assert!(paths(&bundle, None, None, base).is_err());
        assert!(paths(&bundle, None, None, &p.state.join("j")).is_err());
        let pw = jails_dir.join("pw");
        std::fs::write(&pw, "x").unwrap();
        assert!(paths(&bundle, Some(&pw), None, &jails_dir).is_err());
        assert!(paths(&base.join("missing"), None, None, &jails_dir).is_err());
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
        let p = paths(&bundle, None, None, &base.join("jails")).unwrap();
        let prot = protected(&p);
        let j = jails(&p.jail_dir, ["helper", "writer"], &prot).unwrap();
        assert_eq!(j.len(), 2);
        assert!(j["helper"].ends_with("jails/helper") && j["helper"].is_dir());
        // The state folder can never be an agent's folder.
        assert!(Jail::open(&p.state, &prot).is_err());
    }
}
