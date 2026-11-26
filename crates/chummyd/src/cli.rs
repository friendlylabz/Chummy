//! The command line. There's deliberately no way to give the password on
//! it (other programs can read a command line).

use std::path::PathBuf;
use std::time::Duration;

pub const USAGE: &str = "\
chummyd: runs Chummy agents as configured in the Chummy app.

Usage:
  chummyd run --bundle <file> --jail-dir <folder> [--password-file <file>] [--state <folder>] [--grace <seconds>]
  chummyd check --bundle <file> [--password-file <file>]
  chummyd --version
  chummyd --help

  --bundle         the bundle made in the app (Configure chummyd)
  --jail-dir       where the agents' folders go: <jail-dir>/<agent>/
  --password-file  read the bundle's password from this file (a Docker
                   secret, say) instead of asking in the terminal
  --state          the state folder (default: the bundle's path + .state)
  --grace          how long shutting down waits for runs to pause (default 90)

The configuration can't change while chummyd runs. To change it, make a new
bundle in the app and start chummyd again.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Run(RunArgs),
    Check(CheckArgs),
    Help,
    Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArgs {
    pub bundle: PathBuf,
    pub jail_dir: PathBuf,
    pub password_file: Option<PathBuf>,
    pub state: Option<PathBuf>,
    pub grace: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckArgs {
    pub bundle: PathBuf,
    pub password_file: Option<PathBuf>,
}

/// Default time shutting down waits for runs to pause.
pub const GRACE: Duration = Duration::from_secs(90);

pub fn parse(args: &[String]) -> Result<Cmd, String> {
    let Some(cmd) = args.first() else {
        return Ok(Cmd::Help);
    };
    let cmd = match cmd.as_str() {
        "-h" | "--help" | "help" => return Ok(Cmd::Help),
        "-V" | "--version" | "version" => return Ok(Cmd::Version),
        "run" | "check" => cmd.as_str(),
        other => return Err(format!("unknown command “{other}”")),
    };
    let mut flags: Vec<(String, String)> = vec![];
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let (name, value) = match a.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_string(), v.to_string()),
            _ if a.starts_with("--") => (
                a.clone(),
                it.next()
                    .cloned()
                    .ok_or_else(|| format!("{a} needs a value"))?,
            ),
            _ => return Err(format!("unexpected “{a}”")),
        };
        let allowed: &[&str] = if cmd == "run" {
            &[
                "--bundle",
                "--jail-dir",
                "--password-file",
                "--state",
                "--grace",
            ]
        } else {
            &["--bundle", "--password-file"]
        };
        if !allowed.contains(&name.as_str()) {
            return Err(format!("{cmd} doesn't take {name}"));
        }
        if flags.iter().any(|(n, _)| *n == name) {
            return Err(format!("{name} is given twice"));
        }
        if value.trim().is_empty() {
            return Err(format!("{name} needs a value"));
        }
        flags.push((name, value));
    }
    let get = |n: &str| {
        flags
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| PathBuf::from(v))
    };
    let bundle = get("--bundle").ok_or("--bundle is needed")?;
    let password_file = get("--password-file");
    if cmd == "check" {
        return Ok(Cmd::Check(CheckArgs {
            bundle,
            password_file,
        }));
    }
    let grace = match flags.iter().find(|(k, _)| k == "--grace") {
        Some((_, v)) => match v.parse::<u64>() {
            Ok(s) if (1..=3600).contains(&s) => Duration::from_secs(s),
            _ => return Err("--grace is a number of seconds, 1 to 3600".into()),
        },
        None => GRACE,
    };
    Ok(Cmd::Run(RunArgs {
        bundle,
        jail_dir: get("--jail-dir").ok_or("--jail-dir is needed")?,
        password_file,
        state: get("--state"),
        grace,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn the_command_line_is_strict() {
        assert_eq!(parse(&[]).unwrap(), Cmd::Help);
        assert_eq!(parse(&args("--version")).unwrap(), Cmd::Version);
        let Cmd::Run(r) = parse(&args("run --bundle b.chummyd --jail-dir=/srv/j")).unwrap() else {
            panic!()
        };
        assert_eq!(r.bundle, PathBuf::from("b.chummyd"));
        assert_eq!(r.jail_dir, PathBuf::from("/srv/j"));
        assert_eq!((r.state, r.grace), (None, GRACE));
        assert!(parse(&args("run --bundle b")).is_err(), "jail dir needed");
        assert!(parse(&args("run --bundle b --jail-dir j --password hunter2")).is_err());
        assert!(parse(&args("run --bundle b --bundle c --jail-dir j")).is_err());
        assert!(parse(&args("run --bundle b --jail-dir j --grace 0")).is_err());
        assert!(parse(&args("check --bundle b --jail-dir j")).is_err());
        assert!(parse(&args("start")).is_err());
        assert!(matches!(
            parse(&args("check --bundle b --password-file p")).unwrap(),
            Cmd::Check(_)
        ));
    }
}
