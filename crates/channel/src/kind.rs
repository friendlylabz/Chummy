//! Kinds of channel: the one place a kind of channel (Matrix, Telegram…)
//! is described. Everything else (the app's Settings → Channels, chummyd's
//! bundle and terminal, Configure chummyd) works from a [`ChannelKind`]
//! and never names a kind, so adding one is writing one implementation
//! and listing it in [`kinds`].
//!
//! A kind says:
//! - what to ask when one is set up ([`ChannelKind::fields`]): the forms
//!   in the app and Configure chummyd are drawn from these, and options
//!   and secrets are stored by them (secrets in the keychain or the
//!   bundle, options in the settings or the bundle);
//! - whether the answers are right ([`ChannelKind::check`]), the account
//!   it logs in as ([`ChannelKind::identity`], never shared), how to
//!   describe it ([`ChannelKind::summary`], [`ChannelKind::cautions`]) and
//!   what letting people reach an agent through it means
//!   ([`ChannelKind::reach`], [`ChannelKind::widens`]);
//! - who may talk through it ([`ChannelKind::people`]), whether people join
//!   by invitation ([`ChannelKind::invites`]) and can be removed
//!   ([`ChannelKind::removes_people`]);
//! - how to run it ([`ChannelKind::start`]): its messages, and its own way
//!   of verifying people, which it asks the program about with
//!   [`crate::Event::Verify`] (see [`crate::host`]).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value};
use zeroize::Zeroizing;

use crate::{ChannelHost, Handle, Person};

/// A channel's options (not secret), by field key.
pub type Options = Map<String, Value>;

/// A channel's secrets, by field key.
pub type Secrets = BTreeMap<String, Zeroizing<String>>;

/// How a field is answered.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "input", rename_all = "snake_case")]
pub enum Input {
    Text,
    /// A server's address: https, or plain http only on this computer or
    /// the local network.
    Url,
    /// Kept as a secret (the keychain, the bundle), never shown again.
    Secret,
    /// A list, one per line.
    Lines,
    Number {
        min: u32,
        max: u32,
    },
    Toggle,
}

/// Something a kind of channel asks when one is set up.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    #[serde(flatten)]
    pub input: Input,
    pub hint: &'static str,
    pub placeholder: &'static str,
    /// What a new channel starts with.
    pub default: Value,
    pub required: bool,
    /// For addresses: a path that answers when the server is the right
    /// kind (so the forms can check it); empty: none.
    pub probe: &'static str,
}

impl Field {
    pub fn new(key: &'static str, label: &'static str, input: Input) -> Self {
        let default = match input {
            Input::Toggle => Value::Bool(false),
            Input::Number { min, .. } => Value::from(min),
            Input::Lines => Value::Array(vec![]),
            _ => Value::String(String::new()),
        };
        Self {
            key,
            label,
            input,
            hint: "",
            placeholder: "",
            default,
            required: false,
            probe: "",
        }
    }
    pub fn probe(mut self, path: &'static str) -> Self {
        self.probe = path;
        self
    }
    pub fn hint(mut self, h: &'static str) -> Self {
        self.hint = h;
        self
    }
    pub fn placeholder(mut self, p: &'static str) -> Self {
        self.placeholder = p;
        self
    }
    pub fn default(mut self, v: impl Into<Value>) -> Self {
        self.default = v.into();
        self
    }
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }
}

/// A channel as set up: what checks and descriptions read.
pub struct Setup<'a> {
    /// The agents' names, as a sentence reads them ("Helper", "Helper and
    /// Writer").
    pub agent: &'a str,
    pub options: &'a Options,
    /// Its secrets as they'll be used (typed now, or saved).
    pub secrets: &'a Secrets,
    /// The program ("Chummy", "chummyd").
    pub program: &'a str,
}

impl Setup<'_> {
    pub fn text(&self, key: &str) -> &str {
        self.options.get(key).and_then(Value::as_str).unwrap_or("")
    }
    pub fn lines(&self, key: &str) -> Vec<String> {
        lines(self.options.get(key))
    }
    pub fn number(&self, key: &str) -> u32 {
        self.options
            .get(key)
            .and_then(Value::as_u64)
            .map(|n| n.min(u64::from(u32::MAX)) as u32)
            .unwrap_or(0)
    }
    pub fn toggle(&self, key: &str) -> bool {
        self.options
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
    pub fn secret(&self, key: &str) -> &str {
        self.secrets.get(key).map(|s| s.as_str()).unwrap_or("")
    }
}

fn lines(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(s)) => s
            .split(|c: char| c == '\n' || c == ',' || c.is_whitespace())
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
        _ => vec![],
    }
}

/// What a running channel gets.
pub struct Start {
    /// The channel's id (its conversations carry it, and keep each
    /// person's chat apart from other channels').
    pub channel: String,
    /// The agents people talk to (one, unless the kind takes several:
    /// [`ChannelKind::many_agents`]).
    pub agents: Vec<String>,
    pub options: Options,
    pub secrets: Secrets,
    /// A folder of its own, only its owner can open (for stores).
    pub folder: PathBuf,
    /// A key of its own, kept by the program (for encrypting its stores).
    pub key: Zeroizing<[u8; 32]>,
    /// The program ("Chummy", "chummyd").
    pub program: String,
    /// Which one, as people see it ("Chummy", chummyd's instance name).
    pub place: String,
}

impl Start {
    /// The options and secrets, as checks read them.
    pub fn setup<'a>(&'a self, agent: &'a str) -> Setup<'a> {
        Setup {
            agent,
            options: &self.options,
            secrets: &self.secrets,
            program: &self.program,
        }
    }
}

/// A kind of channel. See the module.
pub trait ChannelKind: Send + Sync + 'static {
    /// Its id in settings and bundles ("matrix").
    fn id(&self) -> &'static str;
    /// Its name ("Matrix").
    fn label(&self) -> &'static str;
    /// What it is, for the form.
    fn about(&self) -> &'static str;
    /// What to ask when one is set up, in order.
    fn fields(&self) -> Vec<Field>;
    /// One channel can talk for several agents (one room each, say).
    /// Otherwise each channel is for one agent.
    fn many_agents(&self) -> bool {
        false
    }
    /// People join by invitation (a code made in the program): the
    /// button's words ("Add a person").
    fn invites(&self) -> Option<&'static str> {
        None
    }
    /// People let in can be removed in the program (rather than by
    /// changing the options).
    fn removes_people(&self) -> bool {
        false
    }
    /// What's particular to it to check, after the fields themselves
    /// ([`check`] checks those).
    fn check(&self, s: &Setup) -> Result<(), String>;
    /// The account it logs in as (a bot's id): two channels never share one.
    fn identity(&self, s: &Setup) -> Option<String>;
    /// One line about it, with no secrets ("@bot:example.org on https://…").
    fn summary(&self, s: &Setup) -> String;
    /// What letting people talk to the agent through it means, for the
    /// confirmation asked first.
    fn reach(&self, s: &Setup) -> String;
    /// Whether a change lets more people (or other people) reach the agent
    /// (asks again).
    fn widens(&self, old: &Setup, new: &Setup) -> bool;
    /// Worth a look, as chummyd check lists them.
    fn cautions(&self, _s: &Setup) -> Vec<String> {
        vec![]
    }
    /// What deleting one leaves behind, for the confirmation.
    fn deleting(&self) -> &'static str;
    /// Who may talk through it now (from the options, or what it kept).
    fn people(&self, s: &Setup, kept: Option<&Value>) -> Vec<Person>;
    /// Whether what a field's server sent back to its probe (see
    /// [`Field::probe`]) means it's the right kind of server.
    fn answers(&self, _field: &str, status: u16, _body: &str) -> bool {
        (200..300).contains(&status)
    }
    /// Whether `id` names someone a job can report to (one of the people,
    /// or someone who may be let in later).
    fn valid_person(&self, s: &Setup, id: &str) -> bool;
    /// Runs it.
    fn start(&self, host: Arc<dyn ChannelHost>, start: Start) -> Result<Handle, String>;
}

/// Every kind of channel there is.
pub fn kinds() -> &'static [&'static dyn ChannelKind] {
    &[&crate::matrix::Matrix, &crate::telegram::Telegram]
}

/// The kind of channel with this id.
pub fn kind(id: &str) -> Option<&'static dyn ChannelKind> {
    kinds().iter().copied().find(|k| k.id() == id)
}

/// A kind of channel as the windows draw it.
#[derive(Debug, Clone, Serialize)]
pub struct KindInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub about: &'static str,
    pub fields: Vec<Field>,
    pub many_agents: bool,
    pub invites: Option<&'static str>,
    pub removes_people: bool,
}

/// Checks that the server at `url` (a kind's address field with a probe)
/// answers as it should. Returns what to tell.
pub async fn probe(kind: &str, field: &str, url: &str) -> Result<String, String> {
    let nothing = "There's nothing to check for that.";
    let kind = self::kind(kind).ok_or(nothing)?;
    let f = kind
        .fields()
        .into_iter()
        .find(|f| f.key == field)
        .filter(|f| f.input == Input::Url && !f.probe.is_empty())
        .ok_or(nothing)?;
    let url = url.trim().trim_end_matches('/');
    if let Some(p) = chummy_llm::endpoint_problem(url) {
        return Err(p);
    }
    if chummy_llm::unencrypted_remote(url) {
        return Err(
            "It must use https (plain http only on this computer or your local network).".into(),
        );
    }
    let client = chummy_http::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| chummy_http::describe(&e))?;
    let r = client
        .get(format!("{url}{}", f.probe))
        .send()
        .await
        .map_err(|e| {
            format!(
                "It didn't answer: {}",
                chummy_http::describe(&e.without_url())
            )
        })?;
    let status = r.status();
    let body = r.text().await.unwrap_or_default();
    if kind.answers(field, status.as_u16(), &body) {
        Ok(format!("{} answers.", f.label))
    } else {
        Err(format!(
            "That doesn't look like the right server (it answered {status})."
        ))
    }
}

/// Every kind of channel, for the windows.
pub fn info() -> Vec<KindInfo> {
    kinds()
        .iter()
        .map(|k| KindInfo {
            id: k.id(),
            label: k.label(),
            about: k.about(),
            fields: k.fields(),
            many_agents: k.many_agents(),
            invites: k.invites(),
            removes_people: k.removes_people(),
        })
        .collect()
}

/// Options tidied the same way everywhere: text trimmed (and addresses
/// without a trailing slash), lists as lists without blanks or repeats,
/// numbers as numbers, and anything the kind doesn't ask dropped. Missing
/// ones get their default.
pub fn tidy(kind: &dyn ChannelKind, options: &Options) -> Options {
    let mut out = Options::new();
    for f in kind.fields() {
        if f.input == Input::Secret {
            continue;
        }
        let v = options.get(f.key);
        let tidied = match f.input {
            Input::Text => Value::String(
                v.and_then(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|| f.default.as_str().unwrap_or("").to_string()),
            ),
            Input::Url => Value::String(
                v.and_then(Value::as_str)
                    .map(|s| s.trim().trim_end_matches('/').to_string())
                    .unwrap_or_else(|| f.default.as_str().unwrap_or("").to_string()),
            ),
            Input::Lines => {
                let mut l = match v {
                    Some(v) => lines(Some(v)),
                    None => lines(Some(&f.default)),
                };
                l.sort();
                l.dedup();
                Value::from(l)
            }
            Input::Number { .. } => match v {
                Some(Value::Number(n)) => Value::Number(n.clone()),
                Some(Value::String(s)) => s
                    .trim()
                    .parse::<u64>()
                    .map(Value::from)
                    .unwrap_or(Value::from(0)),
                _ => f.default.clone(),
            },
            Input::Toggle => v
                .and_then(Value::as_bool)
                .map(Value::Bool)
                .unwrap_or_else(|| f.default.clone()),
            Input::Secret => unreachable!(),
        };
        out.insert(f.key.into(), tidied);
    }
    out
}

/// Agents' names as a sentence reads them: "A", "A and B", "A, B and C".
pub fn names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Checks the agents a channel talks for: at least one, and one only
/// unless its kind takes several.
pub fn check_agents(kind: &dyn ChannelKind, agents: &[String]) -> Result<(), String> {
    match agents.len() {
        0 => Err("Choose the agent people talk to.".into()),
        1 => Ok(()),
        _ if kind.many_agents() => Ok(()),
        _ => Err(format!(
            "A {} channel talks for one agent: make one for each.",
            kind.label()
        )),
    }
}

/// Checks a channel: its fields (required ones answered, numbers in range,
/// addresses that are allowed), then what's particular to its kind.
pub fn check(kind: &dyn ChannelKind, s: &Setup) -> Result<(), String> {
    for f in kind.fields() {
        let missing = match f.input {
            Input::Secret => s.secret(f.key).trim().is_empty(),
            Input::Lines => s.lines(f.key).is_empty(),
            Input::Text | Input::Url => s.text(f.key).is_empty(),
            Input::Number { .. } | Input::Toggle => false,
        };
        if missing {
            if f.required {
                return Err(format!("Enter {}.", lowercase_first(f.label)));
            }
            continue;
        }
        match f.input {
            Input::Url => {
                let url = s.text(f.key);
                if let Some(p) = chummy_llm::endpoint_problem(url) {
                    return Err(format!("{}: {p}", f.label));
                }
                if chummy_llm::unencrypted_remote(url) {
                    return Err(format!(
                        "{} must use https (plain http only on this computer or your local network).",
                        f.label
                    ));
                }
            }
            Input::Number { min, max } => {
                let n = s.number(f.key);
                if !(min..=max).contains(&n) {
                    return Err(format!("{} must be between {min} and {max}.", f.label));
                }
            }
            _ => {}
        }
    }
    kind.check(s)
}

fn lowercase_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Secrets as plain strings (as a bundle keeps them), as [`Secrets`].
pub fn secrets_of(map: &BTreeMap<String, String>) -> Secrets {
    map.iter()
        .map(|(k, v)| (k.clone(), Zeroizing::new(v.clone())))
        .collect()
}

/// A bundle's checks ([`chummy_bundle::Config::check`]), with its channels
/// checked by their kinds: what they take, accounts used twice, and who
/// jobs report to. What chummyd and Configure chummyd use.
pub fn check_bundle(c: &chummy_bundle::Config) -> chummy_bundle::Report {
    let mut r = c.check();
    let mut accounts = std::collections::BTreeSet::new();
    for ch in &c.channels.list {
        let Some(k) = kind(&ch.kind) else {
            r.errors.push(format!(
                "“{}” is a kind of channel this version doesn't know ({}).",
                ch.name, ch.kind
            ));
            continue;
        };
        if let Err(e) = check_agents(k, &ch.agents) {
            r.errors.push(format!("“{}”: {e}", ch.name));
            continue;
        }
        let agent = names(
            &ch.agents
                .iter()
                .map(|id| {
                    c.agents
                        .iter()
                        .find(|a| a.id == *id)
                        .map(|a| a.name.clone())
                        .unwrap_or_else(|| id.clone())
                })
                .collect::<Vec<_>>(),
        );
        let options = tidy(k, &ch.options);
        let secrets = secrets_of(&ch.secrets);
        let s = Setup {
            agent: &agent,
            options: &options,
            secrets: &secrets,
            program: "chummyd",
        };
        if let Err(e) = check(k, &s) {
            r.errors.push(format!("“{}”: {e}", ch.name));
            continue;
        }
        if let Some(id) = k.identity(&s)
            && !accounts.insert(id)
        {
            r.errors.push(format!(
                "“{}” uses the same account as another channel: give each its own.",
                ch.name
            ));
        }
        for (job, who) in &ch.reports {
            if !k.valid_person(&s, who) {
                let name = c
                    .jobs
                    .iter()
                    .find(|j| j.id == *job)
                    .map(|j| j.name.as_str())
                    .unwrap_or(job);
                r.errors.push(format!(
                    "The job “{name}” reports to {who}, who can't talk to {agent} in “{}”.",
                    ch.name
                ));
            }
        }
    }
    r
}

/// A key of a channel's own, from a key the program keeps (chummyd's state
/// key) and the channel's id.
pub fn derive_key(key: &[u8], channel: &str) -> Zeroizing<[u8; 32]> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"chummy channel key\0");
    h.update(key);
    h.update(channel.as_bytes());
    Zeroizing::new(h.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_listed_once_with_secrets_apart() {
        let ids: Vec<&str> = kinds().iter().map(|k| k.id()).collect();
        let mut sorted = ids.clone();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len());
        for k in kinds() {
            assert!(kind(k.id()).is_some());
            let o = tidy(*k, &Options::new());
            for f in k.fields() {
                assert_eq!(o.contains_key(f.key), f.input != Input::Secret, "{}", f.key);
            }
        }
    }

    #[test]
    fn options_are_tidied() {
        let k = crate::matrix::Matrix;
        let mut o = Options::new();
        o.insert("homeserver".into(), " https://hs.example.org/ ".into());
        o.insert("people".into(), "@b:hs\n@a:hs, @a:hs".into());
        o.insert("upload_mb".into(), "50".into());
        o.insert("stray".into(), true.into());
        let t = tidy(&k, &o);
        assert_eq!(t["homeserver"], "https://hs.example.org");
        assert_eq!(t["people"], serde_json::json!(["@a:hs", "@b:hs"]));
        assert_eq!(t["upload_mb"], 50);
        assert!(!t.contains_key("stray"));
        assert!(!t.contains_key("password"));
    }
}
