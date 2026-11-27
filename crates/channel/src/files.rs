//! Files people send over a channel: saved in the agent's folder under
//! `uploads/`, through its jail (by the host:
//! [`crate::ChannelHost::save_upload`]). Sending a file doesn't start a
//! run, and agents can't send files out.

/// Where files go, inside the agent's folder.
pub const UPLOADS: &str = "uploads";

/// The longest file name kept, in characters.
const MAX_NAME: usize = 120;

/// A file name that's safe on every system: no folders, no control or
/// reserved characters, no leading dots, not a reserved Windows name, and
/// not too long (keeping the extension).
pub fn clean_name(name: &str) -> String {
    // Only the last part of a path.
    let name = name.rsplit(['/', '\\']).next().unwrap_or("");
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    out = out
        .trim()
        .trim_start_matches('.')
        .trim_end_matches(['.', ' '])
        .to_string();
    if out.chars().count() > MAX_NAME {
        let (stem, ext) = split_ext(&out);
        let ext: String = ext.chars().take(20).collect();
        let keep = MAX_NAME.saturating_sub(ext.chars().count());
        out = format!("{}{ext}", stem.chars().take(keep).collect::<String>());
    }
    let stem = split_ext(&out).0.to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        out = format!("_{out}");
    }
    if out.is_empty() {
        out = "file".into();
    }
    out
}

/// `("report", ".pdf")`; no extension for names like `.bashrc` or `notes`.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// The first of `name`, `name (2)`, `name (3)`… that `taken` says is free.
pub fn unique(name: &str, taken: impl Fn(&str) -> bool) -> Option<String> {
    if !taken(name) {
        return Some(name.to_string());
    }
    let (stem, ext) = split_ext(name);
    (2..1000)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|n| !taken(n))
}

/// Saves `data` as `uploads/<name>` in an agent's folder, through its jail,
/// with a name that isn't taken yet. Returns the name it got. For
/// [`crate::ChannelHost::save_upload`].
pub fn save_to_folder<P: chummy_session::Platform>(
    p: &P,
    agent: &str,
    name: &str,
    data: &[u8],
) -> Result<String, String> {
    use chummy_agent::AgentConfig;
    let path = p
        .config()
        .agent(agent)
        .map(|a| a.workspace.path.clone())
        .ok_or("That agent no longer exists.")?;
    let jail = p.open_jail(&path)?;
    jail.create_dir(UPLOADS).map_err(|e| e.to_string())?;
    let free = unique(name, |n| {
        jail.exists(&format!("{UPLOADS}/{n}")).unwrap_or(true)
    })
    .ok_or("too many files by that name")?;
    jail.write(&format!("{UPLOADS}/{free}"), data)
        .map_err(|e| e.to_string())?;
    Ok(free)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe() {
        assert_eq!(clean_name("report.pdf"), "report.pdf");
        assert_eq!(clean_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_name("C:\\Users\\me\\a.txt"), "a.txt");
        assert_eq!(clean_name(".bashrc"), "bashrc");
        assert_eq!(clean_name("a<b>:c?.txt"), "a_b__c_.txt");
        assert_eq!(clean_name("line\nbreak"), "line_break");
        assert_eq!(clean_name("con.txt"), "_con.txt");
        assert_eq!(clean_name("COM1"), "_COM1");
        assert_eq!(clean_name("comma.txt"), "comma.txt");
        assert_eq!(clean_name("..."), "file");
        assert_eq!(clean_name(""), "file");
        assert_eq!(clean_name("name. "), "name");
        let long = format!("{}.tar.gz", "x".repeat(300));
        let c = clean_name(&long);
        assert_eq!(c.chars().count(), MAX_NAME);
        assert!(c.ends_with(".gz"), "{c}");
    }

    #[test]
    fn names_are_never_reused() {
        let taken = ["a.txt", "a (2).txt"];
        assert_eq!(
            unique("a.txt", |n| taken.contains(&n)).unwrap(),
            "a (3).txt"
        );
        assert_eq!(unique("b.txt", |n| taken.contains(&n)).unwrap(), "b.txt");
        assert_eq!(unique("notes", |n| n == "notes").unwrap(), "notes (2)");
        assert_eq!(unique("x", |_| true), None);
    }
}
