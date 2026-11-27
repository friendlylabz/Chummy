use std::collections::BTreeMap;

use super::*;
use chummy_flow::{Block, Link, LoopFile, Position};

fn echo_loop() -> LoopFile {
    let b = |id: &str, kind: &str| Block {
        id: id.into(),
        kind: kind.into(),
        label: id.into(),
        position: Position::default(),
        params: serde_json::json!({}),
        comment: String::new(),
    };
    LoopFile {
        blocks: vec![b("in", "input"), b("out", "output")],
        links: vec![Link::new(("in", "out"), ("out", "in"))],
        ..LoopFile::new("echo")
    }
}

fn config() -> Config {
    let mut c = Config::new_instance("Home server", "America/New_York").unwrap();
    c.providers.push(
        serde_json::from_value(serde_json::json!({
            "id": "m", "name": "Local server", "kind": "open_ai_compatible",
            "base_url": "http://127.0.0.1:8080/v1", "model": "x"
        }))
        .unwrap(),
    );
    c.secrets.insert("provider:m".into(), "sk-secret".into());
    c.loops.insert("echo".into(), echo_loop());
    c.agents.push(BundleAgent {
        id: "helper".into(),
        name: "Helper".into(),
        loop_id: "echo".into(),
        model: Some("m".into()),
        tools: Default::default(),
        loop_config: Default::default(),
        model_options: Default::default(),
    });
    c.channels.terminal.enabled = true;
    c
}

const PASSWORD: &str = "correct-horse-battery-staple-42";

fn key(c: &Config) -> BundleKey {
    BundleKey::from_password(PASSWORD, &c.instance.id).unwrap()
}

#[test]
fn a_sealed_bundle_opens_only_with_its_password() {
    let c = config();
    assert_eq!(c.check(), Report::default());
    let sealed = seal(&c, PASSWORD).unwrap();
    // Nothing secret is readable in the file.
    let text = String::from_utf8_lossy(&sealed);
    assert!(!text.contains("sk-secret") && !text.contains("Helper"));
    assert_eq!(header(&sealed).unwrap().instance_id, c.instance.id);
    assert_eq!(open(&sealed, &key(&c)).unwrap(), c);
    let wrong = BundleKey::for_bundle(&sealed, "wrong password!!").unwrap();
    assert_eq!(open(&sealed, &wrong), Err(BundleError::WrongPassword));

    // Any changed byte (header or ciphertext) and it doesn't open.
    for at in [20, sealed.len() - 1] {
        let mut bad = sealed.clone();
        bad[at] ^= 1;
        assert!(open(&bad, &key(&c)).is_err(), "byte {at}");
    }
    assert_eq!(open(b"hello", &key(&c)), Err(BundleError::NotABundle));
    assert_eq!(open(&[], &key(&c)), Err(BundleError::NotABundle));
    // Version 1 bundles are refused, saying so.
    let mut old = sealed.clone();
    old[7] = 1;
    assert_eq!(open(&old, &key(&c)), Err(BundleError::Old));
}

#[test]
fn the_password_file_holds_a_key_never_the_password() {
    let c = config();
    let k = key(&c);
    let line = k.to_line();
    assert!(line.starts_with("chummyd-key-2 ") && line.contains(&c.instance.id));
    assert!(!line.contains(PASSWORD));
    assert_eq!(BundleKey::from_line(&format!("{}\n", *line)).unwrap(), k);
    // A password, or anything else, is refused.
    for not_a_key in [PASSWORD, "", "chummyd-key-2 x 00", "chummyd-key-1 a 00"] {
        assert!(BundleKey::from_line(not_a_key).is_err(), "{not_a_key}");
    }

    // Every bundle of the instance with that password opens with it, so
    // the file outlives new exports.
    let mut changed = c.clone();
    changed.instance.name = "Renamed".into();
    let sealed = seal(&changed, PASSWORD).unwrap();
    assert_eq!(open(&sealed, &k).unwrap().instance.name, "Renamed");
    assert_eq!(
        seal_with(&changed, &k).map(|b| open(&b, &k).is_ok()),
        Ok(true)
    );

    // Another instance's key doesn't open it.
    let other = Config::new_instance("Other", "UTC").unwrap();
    let theirs = BundleKey::from_password(PASSWORD, &other.instance.id).unwrap();
    assert!(matches!(
        open(&sealed, &theirs),
        Err(BundleError::OtherInstance(_))
    ));
    assert_ne!(theirs.to_line(), k.to_line());
}

#[test]
fn weak_passwords_and_bad_configurations_are_refused() {
    let c = config();
    assert!(matches!(seal(&c, "short"), Err(BundleError::Other(_))));
    assert!(password_problem("aaaaaaaaaaaaaaaaaaaa").is_some());
    assert!(password_problem("password1234").is_some());
    assert!(password_problem(PASSWORD).is_none());
    let generated = generate_password().unwrap();
    assert_eq!(generated.len(), 29);
    assert!(password_problem(&generated).is_none(), "{generated}");
    assert_ne!(generated, generate_password().unwrap());

    let mut bad = c.clone();
    bad.agents[0].loop_id = "missing".into();
    bad.providers.push(
        serde_json::from_value(serde_json::json!({
            "id": "eng", "name": "Built-in", "kind": "local", "model": "x"
        }))
        .unwrap(),
    );
    bad.instance.time_zone = "Nowhere/Land".into();
    let r = bad.check();
    assert_eq!(r.errors.len(), 3, "{r:?}");
    assert!(matches!(seal(&bad, PASSWORD), Err(BundleError::Invalid(e)) if e.len() == 3));

    // The chat limits are the bundle's to set, within bounds.
    let mut limits = c.clone();
    limits.limits.chat_messages = 50_000;
    limits.limits.chats = 500;
    assert!(limits.check().ok());
    limits.limits.chat_messages = 5;
    limits.limits.chats = 0;
    let r = limits.check();
    assert_eq!(r.errors.len(), 2, "{r:?}");
    assert!(
        r.errors
            .iter()
            .any(|e| e.contains("Messages kept per chat"))
    );
}

#[test]
fn warnings_say_what_wont_work() {
    let mut c = config();
    c.channels.terminal.enabled = false;
    c.permissions.tools = true;
    c.agents[0].tools.insert(
        "calculator".into(),
        chummy_agent::AgentTool {
            ask: true,
            network: false,
        },
    );
    let r = c.check();
    assert!(r.ok());
    assert_eq!(r.warnings.len(), 2, "{r:?}");
    // A channel for it that takes approvals removes that warning (and the
    // one about no channel).
    c.channels.list.push(channel("helper"));
    assert_eq!(c.check().warnings.len(), 1);
    c.channels.list[0].approvals = true;
    assert_eq!(c.check().warnings.len(), 0);
}

#[test]
fn unknown_fields_and_other_instances_are_refused() {
    let c = config();
    let mut v = serde_json::to_value(&c).unwrap();
    v["surprise"] = true.into();
    assert!(serde_json::from_value::<Config>(v).is_err());
    let key = c.state_key_bytes().unwrap();
    assert_eq!(key.len(), 32);
    let mut other = c.clone();
    other.state_key = "short".into();
    assert!(!other.check().ok());
}

fn channel(agent: &str) -> BundleChannel {
    BundleChannel {
        id: "phone".into(),
        name: "Phone".into(),
        kind: "matrix".into(),
        agents: vec![agent.into()],
        approvals: false,
        options: serde_json::Map::new(),
        secrets: BTreeMap::new(),
        reports: BTreeMap::new(),
    }
}

#[test]
fn channels_are_checked_for_their_shape() {
    let mut c = config();
    c.channels.list.push(channel("helper"));
    c.channels.list[0]
        .secrets
        .insert("password".into(), "secret".into());
    let r = c.check();
    assert!(r.ok(), "{r:?}");
    let sealed = seal(&c, PASSWORD).unwrap();
    assert_eq!(open(&sealed, &key(&c)).unwrap(), c);

    // The same id twice, an agent that isn't there, a job that isn't.
    let mut other = channel("gone");
    other
        .reports
        .insert("nope".into(), "@me:example.org".into());
    c.channels.list.push(other);
    let r = c.check();
    assert_eq!(r.errors.len(), 3, "{r:?}");

    assert!(valid_matrix_id("@alice:localhost:8008"));
    assert!(!valid_matrix_id("@:x") && !valid_matrix_id("alice:x") && !valid_matrix_id("@a b:x"));
}

#[test]
fn only_this_configuration_version_opens() {
    let mut c = config();
    assert!(c.check().ok());
    for other in [2, CONFIG_VERSION + 1] {
        c.version = other;
        assert!(!c.check().ok(), "version {other}");
    }
}
