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
        network: false,
        loop_config: Default::default(),
        model_options: Default::default(),
        channel_approvals: false,
    });
    c.channels.terminal.enabled = true;
    c
}

const PASSWORD: &str = "correct-horse-battery-staple-42";

#[test]
fn a_sealed_bundle_opens_only_with_its_password() {
    let c = config();
    assert_eq!(c.check(), Report::default());
    let sealed = seal(&c, PASSWORD).unwrap();
    // Nothing secret is readable in the file.
    let text = String::from_utf8_lossy(&sealed);
    assert!(!text.contains("sk-secret") && !text.contains("Helper"));
    assert_eq!(header(&sealed).unwrap().instance_id, c.instance.id);
    assert_eq!(open(&sealed, PASSWORD).unwrap(), c);
    assert_eq!(
        open(&sealed, "wrong password!!"),
        Err(BundleError::WrongPassword)
    );

    // Any changed byte (header or ciphertext) and it doesn't open.
    for at in [20, sealed.len() - 1] {
        let mut bad = sealed.clone();
        bad[at] ^= 1;
        assert!(open(&bad, PASSWORD).is_err(), "byte {at}");
    }
    assert_eq!(open(b"hello", PASSWORD), Err(BundleError::NotABundle));
    assert_eq!(open(&[], PASSWORD), Err(BundleError::NotABundle));
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
}

#[test]
fn warnings_say_what_wont_work() {
    let mut c = config();
    c.channels.terminal.enabled = false;
    c.permissions.tools = true;
    c.tools.insert(
        "calculator".into(),
        chummy_agent::ToolPref {
            enabled: true,
            network: false,
            ask: true,
        },
    );
    c.agents[0].tools.insert("calculator".into());
    let r = c.check();
    assert!(r.ok());
    assert_eq!(r.warnings.len(), 2, "{r:?}");
    // Allowing approvals over channels removes that warning.
    c.agents[0].channel_approvals = true;
    assert_eq!(c.check().warnings.len(), 1);
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
