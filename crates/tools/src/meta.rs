//! Reading a tool file's header.
//!
//! A tool file is a Python script with a [PEP 723] inline metadata block:
//!
//! ```python
//! # /// script
//! # requires-python = ">=3.14"
//! # dependencies = ["beautifulsoup4==4.13.4"]
//! #
//! # [tool.chummy]
//! # name = "fetch_page"
//! # title = "Fetch web page"
//! # version = "1.0.0"
//! # network = true
//! # timeout = 60
//! # ///
//! ```
//!
//! `requires-python` and `dependencies` are standard PEP 723 fields.
//! Chummy's own settings live in the `[tool.chummy]` table. The header is
//! read without running any of the file's code, so everything a user agrees
//! to (dependencies, internet access) is known before installing.
//!
//! [PEP 723]: https://peps.python.org/pep-0723/

use serde::{Deserialize, Serialize};

use crate::ToolError;

/// What a tool file's header says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolMeta {
    /// The name the model calls: lowercase letters, digits and `_`.
    pub name: String,
    /// Display name.
    pub title: String,
    pub version: String,
    /// The tool wants to use the internet (still off until the user allows it).
    pub network: bool,
    /// Longest a call may run, in seconds.
    pub timeout_secs: u32,
    pub requires_python: Option<String>,
    /// PEP 508 requirements, from PyPI only.
    pub dependencies: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct ScriptBlock {
    requires_python: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    tool: ToolTables,
}

/// `[tool.*]` tables. Other tools' tables are allowed and ignored.
#[derive(Default, Deserialize)]
struct ToolTables {
    chummy: Option<ChummyTable>,
}

fn d_timeout() -> u32 {
    30
}
fn d_version() -> String {
    "0".into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChummyTable {
    name: String,
    #[serde(default)]
    title: String,
    #[serde(default = "d_version")]
    version: String,
    #[serde(default)]
    network: bool,
    #[serde(default = "d_timeout")]
    timeout: u32,
}

/// Longest timeout a tool may ask for.
pub const MAX_TIMEOUT_SECS: u32 = 300;

/// Extracts the `# /// script` block's TOML, following PEP 723's rules.
pub fn script_block(source: &str) -> Result<String, ToolError> {
    let bad = |m: &str| ToolError::Definition(m.into());
    let mut found: Option<String> = None;
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        let line = line.trim_end_matches('\r');
        let Some(kind) = line.strip_prefix("# /// ") else {
            continue;
        };
        let kind = kind.trim_end();
        if kind != "script" {
            continue;
        }
        if found.is_some() {
            return Err(bad("the file has more than one “# /// script” block"));
        }
        let mut body = String::new();
        let mut closed = false;
        for l in lines.by_ref() {
            let l = l.trim_end_matches('\r');
            if l == "# ///" {
                closed = true;
                break;
            }
            if l == "#" {
                body.push('\n');
            } else if let Some(rest) = l.strip_prefix("# ") {
                body.push_str(rest);
                body.push('\n');
            } else {
                return Err(bad(
                    "every line of the “# /// script” block must start with “#”",
                ));
            }
        }
        if !closed {
            return Err(bad("the “# /// script” block isn't closed with “# ///”"));
        }
        found = Some(body);
    }
    found.ok_or_else(|| {
        bad("the file has no “# /// script” header (see the tool format in Chummy's docs)")
    })
}

pub(crate) fn valid_name(name: &str) -> bool {
    let mut c = name.chars();
    matches!(c.next(), Some('a'..='z'))
        && name.len() <= 48
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A plain PyPI requirement: a name, optional extras, version specifiers and
/// an environment marker. Direct references (`pkg @ url`), paths and URLs
/// are refused, so dependencies always come from the package index and
/// match the lock's hashes.
pub(crate) fn valid_requirement(req: &str) -> bool {
    let req = req.trim();
    if req.is_empty() || req.contains('@') || req.contains("://") {
        return false;
    }
    let (spec, _marker) = req.split_once(';').unwrap_or((req, ""));
    let name_end = spec
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
        .unwrap_or(spec.len());
    if name_end == 0 || !spec.as_bytes()[0].is_ascii_alphanumeric() {
        return false;
    }
    let rest = spec[name_end..].trim_start();
    let rest = match rest.strip_prefix('[') {
        Some(r) => match r.split_once(']') {
            Some((extras, after))
                if extras.split(',').all(|e| {
                    let e = e.trim();
                    !e.is_empty()
                        && e.chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                }) =>
            {
                after.trim_start()
            }
            _ => return false,
        },
        None => rest,
    };
    rest.is_empty()
        || (rest.starts_with(['<', '>', '=', '!', '~'])
            && rest.chars().all(|c| {
                c.is_ascii_alphanumeric() || "<>=!~.*, ".contains(c) || c == '+' || c == '-'
            }))
}

/// Reads and checks a tool file's header.
pub fn parse_meta(source: &str) -> Result<ToolMeta, ToolError> {
    let bad = |m: String| ToolError::Definition(m);
    let toml_text = script_block(source)?;
    let block: ScriptBlock = toml::from_str(&toml_text)
        .map_err(|e| bad(format!("the header isn't valid: {}", e.message())))?;
    let c = block
        .tool
        .chummy
        .ok_or_else(|| bad("the header has no [tool.chummy] table with the tool's name".into()))?;
    if !valid_name(&c.name) {
        return Err(bad(format!(
            "the tool name “{}” must start with a lowercase letter and use only a–z, 0–9 and _ (48 at most)",
            c.name
        )));
    }
    if !(1..=MAX_TIMEOUT_SECS).contains(&c.timeout) {
        return Err(bad(format!(
            "timeout must be between 1 and {MAX_TIMEOUT_SECS} seconds"
        )));
    }
    if let Some(r) = block.dependencies.iter().find(|r| !valid_requirement(r)) {
        return Err(bad(format!(
            "the dependency “{r}” isn't a plain package requirement (no URLs or paths)"
        )));
    }
    let title = if c.title.trim().is_empty() {
        c.name.replace('_', " ")
    } else {
        c.title.trim().to_string()
    };
    Ok(ToolMeta {
        name: c.name,
        title,
        version: c.version,
        network: c.network,
        timeout_secs: c.timeout,
        requires_python: block.requires_python,
        dependencies: block.dependencies,
    })
}

fn version_parts(v: &str) -> Option<Vec<u64>> {
    v.trim().split('.').map(|p| p.parse::<u64>().ok()).collect()
}

fn cmp_versions(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    let n = a.len().max(b.len());
    (0..n)
        .map(|i| {
            a.get(i)
                .copied()
                .unwrap_or(0)
                .cmp(&b.get(i).copied().unwrap_or(0))
        })
        .find(|o| o.is_ne())
        .unwrap_or(std::cmp::Ordering::Equal)
}

/// Whether `python` (like "3.14.7") satisfies a `requires-python`
/// specifier set such as ">=3.12,<4" or "==3.14.*".
pub fn python_satisfies(spec: &str, python: &str) -> Result<bool, ToolError> {
    use std::cmp::Ordering::*;
    let have = version_parts(python).unwrap_or_default();
    for clause in spec.split(',').map(str::trim).filter(|c| !c.is_empty()) {
        let op_len = clause
            .find(|c: char| c.is_ascii_digit())
            .ok_or_else(|| ToolError::Definition(format!("can't read requires-python “{spec}”")))?;
        let (op, ver) = clause.split_at(op_len);
        let op = op.trim();
        if let Some(prefix) = ver.strip_suffix(".*") {
            let want = version_parts(prefix).unwrap_or_default();
            let matches = have.len() >= want.len() && have[..want.len()] == want[..];
            let ok = match op {
                "==" => matches,
                "!=" => !matches,
                _ => {
                    return Err(ToolError::Definition(format!(
                        "can't read requires-python “{spec}”"
                    )));
                }
            };
            if !ok {
                return Ok(false);
            }
            continue;
        }
        let want = version_parts(ver)
            .ok_or_else(|| ToolError::Definition(format!("can't read requires-python “{spec}”")))?;
        let o = cmp_versions(&have, &want);
        let ok = match op {
            ">=" => o != Less,
            ">" => o == Greater,
            "<=" => o != Greater,
            "<" => o == Less,
            "==" | "===" => o == Equal,
            "!=" => o != Equal,
            "~=" => {
                let mut upper = want[..want.len().saturating_sub(1)].to_vec();
                if let Some(last) = upper.last_mut() {
                    *last += 1;
                }
                o != Less && cmp_versions(&have, &upper) == Less
            }
            _ => {
                return Err(ToolError::Definition(format!(
                    "can't read requires-python “{spec}”"
                )));
            }
        };
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"# /// script
# requires-python = ">=3.14"
# dependencies = ["beautifulsoup4==4.13.4", "tzdata"]
#
# [tool.chummy]
# name = "fetch_page"
# network = true
# ///
import chummy
"#;

    #[test]
    fn reads_the_header() {
        let m = parse_meta(GOOD).unwrap();
        assert_eq!(m.name, "fetch_page");
        assert_eq!(m.title, "fetch page");
        assert!(m.network);
        assert_eq!(m.timeout_secs, 30);
        assert_eq!(m.dependencies.len(), 2);
    }

    #[test]
    fn refuses_bad_headers() {
        assert!(parse_meta("import chummy").is_err());
        assert!(parse_meta(&GOOD.replace("fetch_page", "Fetch Page")).is_err());
        assert!(
            parse_meta(&GOOD.replace("network = true", "network = true\n# shell = true")).is_err()
        );
        assert!(parse_meta(&GOOD.replace("tzdata", "evil @ https://x.test/e.whl")).is_err());
        assert!(parse_meta(&GOOD.replace("# ///\nimport", "import")).is_err());
        let twice = format!("{GOOD}\n{GOOD}");
        assert!(parse_meta(&twice).is_err());
        let no_table = "# /// script\n# dependencies = []\n# ///\n";
        assert!(parse_meta(no_table).is_err());
    }

    #[test]
    fn requirement_rules() {
        for ok in [
            "bs4",
            "requests[socks]>=2,<3",
            "tzdata==2025.2",
            "x; python_version >= '3.12'",
        ] {
            assert!(valid_requirement(ok), "{ok}");
        }
        for bad in [
            "",
            "pkg @ file:///x",
            "https://x/y.whl",
            "./local",
            "-e .",
            "a$b",
        ] {
            assert!(!valid_requirement(bad), "{bad}");
        }
    }

    #[test]
    fn python_versions() {
        assert!(python_satisfies(">=3.12", "3.14.7").unwrap());
        assert!(!python_satisfies(">=3.15", "3.14.7").unwrap());
        assert!(python_satisfies(">=3.12,<4", "3.14.7").unwrap());
        assert!(python_satisfies("==3.14.*", "3.14.7").unwrap());
        assert!(!python_satisfies("==3.13.*", "3.14.7").unwrap());
        assert!(python_satisfies("~=3.12", "3.14.7").unwrap());
        assert!(!python_satisfies("~=3.12.1", "3.14.7").unwrap());
        assert!(python_satisfies("abc", "3.14").is_err());
    }
}
