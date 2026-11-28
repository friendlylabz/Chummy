//! The Discord channel against a stand-in Discord (web API and Gateway):
//! invitations, talking, answers shown as they're written, approvals with
//! buttons, slash commands, files, servers ignored, reconnecting, catching
//! up after being off, and what goes wrong.

#![cfg(feature = "discord")]

#[path = "support/fake_discord.rs"]
mod fake;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chummy_channel::{
    ChannelHost, Event, FromChannel, Handle, Options, Secrets, Start, Status, ToChannel, Verify,
};
use fake::{Fake, TOKEN, dm, user};
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

#[derive(Default)]
struct Seen {
    statuses: Vec<Status>,
    events: Vec<Event>,
    saved: Vec<(String, Vec<u8>)>,
}

struct TestHost {
    kept: Mutex<Option<Value>>,
    seen: Arc<Mutex<Seen>>,
    tx: UnboundedSender<FromChannel>,
}

impl ChannelHost for TestHost {
    fn agent_name(&self, id: &str) -> Option<String> {
        (id == "helper").then(|| "Helper".to_string())
    }
    fn kept(&self) -> Option<Value> {
        self.kept.lock().unwrap().clone()
    }
    fn keep(&self, kept: Value) {
        *self.kept.lock().unwrap() = Some(kept);
    }
    fn received(&self, m: FromChannel) {
        let _ = self.tx.send(m);
    }
    fn event(&self, e: Event) {
        self.seen.lock().unwrap().events.push(e);
    }
    fn status(&self, s: Status) {
        self.seen.lock().unwrap().statuses.push(s);
    }
    fn save_upload(&self, _agent: &str, name: &str, data: &[u8]) -> Result<String, String> {
        self.seen
            .lock()
            .unwrap()
            .saved
            .push((name.into(), data.to_vec()));
        Ok(name.into())
    }
}

struct Bot {
    fake: Fake,
    handle: Handle,
    seen: Arc<Mutex<Seen>>,
    rx: UnboundedReceiver<FromChannel>,
    host: Arc<TestHost>,
}

/// A Discord channel as the programs start one: through its kind.
fn settings(fake: &Fake, token: &str, stream: bool) -> Start {
    let mut options = Options::new();
    options.insert("api_url".into(), fake.url.clone().into());
    options.insert("upload_mb".into(), 1.into());
    options.insert("stream".into(), stream.into());
    let mut secrets = Secrets::new();
    secrets.insert("token".into(), zeroize::Zeroizing::new(token.into()));
    Start {
        channel: "ch-d".into(),
        agents: vec!["helper".into()],
        options,
        secrets,
        folder: std::env::temp_dir(),
        key: zeroize::Zeroizing::new([7; 32]),
        program: "Chummy".into(),
        place: "Chummy".into(),
    }
}

fn verify_n(e: &Event) -> Option<(u32, String)> {
    match e {
        Event::Verify(Verify {
            n, who, confirm, ..
        }) if confirm.is_some() => Some((*n, who.clone())),
        _ => None,
    }
}

async fn start_with(fake: Fake, token: &str, stream: bool, kept: Option<Value>) -> Bot {
    let (tx, rx) = unbounded_channel();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let host = Arc::new(TestHost {
        kept: Mutex::new(kept),
        seen: seen.clone(),
        tx,
    });
    let kind = chummy_channel::kind("discord").unwrap();
    let handle = kind
        .start(host.clone(), settings(&fake, token, stream))
        .unwrap();
    Bot {
        fake,
        handle,
        seen,
        rx,
        host,
    }
}

async fn until<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    for _ in 0..400 {
        if let Some(v) = f() {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("waited too long for {what}");
}

fn text_of(b: &Value) -> &str {
    b["content"].as_str().unwrap_or("")
}

impl Bot {
    async fn connected(&self) {
        until("connected", || {
            self.seen
                .lock()
                .unwrap()
                .statuses
                .contains(&Status::Connected)
                .then_some(())
        })
        .await;
    }

    fn event(&self, f: impl Fn(&Event) -> bool) -> Option<Event> {
        self.seen
            .lock()
            .unwrap()
            .events
            .iter()
            .find(|e| f(e))
            .cloned()
    }

    fn notes(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Note { text } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    async fn next(&mut self) -> FromChannel {
        tokio::time::timeout(Duration::from_secs(20), self.rx.recv())
            .await
            .expect("something from the channel")
            .unwrap()
    }

    /// The next message passed on (skipping conversation lists).
    async fn next_message(&mut self) -> (chummy_channel::Conv, String, Option<String>) {
        loop {
            if let FromChannel::Message {
                conv,
                text,
                answers,
            } = self.next().await
            {
                return (conv, text, answers);
            }
        }
    }

    /// Lets Alice (id 42) in with an invitation code.
    async fn let_alice_in(&mut self) -> Value {
        let alice = user("42", "alice");
        let code = self.handle.invite().await.unwrap();
        assert!(code.text.contains(&code.code), "{}", code.text);
        self.fake.say(&alice, &code.code);
        let n = until("the invitation", || {
            self.event(|e| verify_n(e).is_some())
                .and_then(|e| verify_n(&e))
                .map(|(n, who)| {
                    assert!(
                        who.contains("Alice (@alice)") && who.contains("42"),
                        "{who}"
                    );
                    n
                })
        })
        .await;
        self.handle.answer(Some(n), true);
        loop {
            if let FromChannel::Ready { convs } = self.next().await
                && convs.len() == 1
            {
                assert_eq!(convs[0].place, "42");
                assert_eq!(convs[0].who, "Alice (@alice)");
                break;
            }
        }
        self.fake
            .wait_call(
                "POST",
                &format!("/api/v10/channels/{}/messages", dm("42")),
                |b| text_of(b).contains("can talk to Helper"),
            )
            .await;
        alice
    }
}

#[tokio::test]
async fn people_ask_with_a_code_and_then_talk() {
    let fake = Fake::start().await;
    let mut bot = start_with(fake.clone(), TOKEN.as_str(), true, None).await;
    bot.connected().await;
    assert!(
        bot.event(|e| *e
            == Event::Account {
                name: "@helper_bot".into()
            })
        .is_some()
    );
    // Slash commands, in direct messages only.
    let cmds = fake
        .wait_call("PUT", "/api/v10/applications/", |_| true)
        .await;
    let cmds = cmds.as_array().unwrap();
    assert!(
        cmds.iter()
            .any(|c| c["name"] == "status" && c["contexts"] == json!([1]))
    );

    // Strangers get nothing: not a hello, not a wrong code.
    let eve = user("66", "eve");
    fake.say(&eve, "hello");
    let code = bot.handle.invite().await.unwrap();
    assert_eq!(code.code.len(), 6);
    assert!(code.text.contains("@helper_bot"), "{}", code.text);
    let wrong = if code.code == "000000" {
        "111111"
    } else {
        "000000"
    };
    fake.say(&eve, wrong);
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(fake.sent_to("66").is_empty(), "{:?}", fake.sent_to("66"));
    assert!(bot.event(|e| verify_n(e).is_some()).is_none());

    // The right code: they ask, and wait to be let in.
    let alice = user("42", "alice");
    fake.say(&alice, &code.code);
    let n = until("the invitation", || {
        bot.event(|e| verify_n(e).is_some())
            .and_then(|e| verify_n(&e))
            .map(|(n, _)| n)
    })
    .await;
    fake.wait_call(
        "POST",
        &format!("/api/v10/channels/{}/messages", dm("42")),
        |b| text_of(b).contains("wait for whoever runs Chummy"),
    )
    .await;
    // A code works once.
    let bob = user("7", "bob");
    fake.say(&bob, &code.code);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(fake.sent_to("7").is_empty());

    bot.handle.answer(Some(n), true);
    until("Alice let in", || {
        bot.event(|e| matches!(e, Event::People { people } if people.len() == 1))
    })
    .await;
    let kept = bot.host.kept().unwrap();
    assert_eq!(chummy_channel::discord::kept_people(&kept)[0].id, "42");

    // Alice talks: her message is passed on, as her conversation.
    fake.say(&alice, "hi there");
    let (conv, text, answers) = bot.next_message().await;
    assert_eq!((conv.place.as_str(), conv.agent.as_str()), ("42", "helper"));
    assert_eq!(conv.chat, chummy_channel::channel_chat("ch-d", "42"));
    assert_eq!((text.as_str(), answers), ("hi there", None));
    // Typed commands, with / or !, and slash commands.
    fake.say(&alice, "!status");
    assert_eq!(bot.next_message().await.1, "/status");
    fake.command(
        &alice,
        "deny",
        json!([{ "name": "reason", "type": 3, "value": "too risky" }]),
    );
    assert_eq!(bot.next_message().await.1, "/deny too risky");
    let cb = fake
        .wait_call("POST", "/api/v10/interactions/", |_| true)
        .await;
    assert_eq!(cb["type"], 4);
    // Messages in servers are never passed on.
    fake.in_server(&alice, "hello server");

    // The answer, as it's written: one message, edited as it grows.
    let out = bot.handle.sender();
    out.send(ToChannel::Started {
        run_id: "r1".into(),
        place: "42".into(),
    })
    .unwrap();
    fake.wait_call(
        "POST",
        &format!("/api/v10/channels/{}/typing", dm("42")),
        |_| true,
    )
    .await;
    out.send(ToChannel::Run {
        run_id: "r1".into(),
        place: "42".into(),
        text: "Thinking about **this**".into(),
        done: false,
    })
    .unwrap();
    let first = fake
        .wait_call(
            "POST",
            &format!("/api/v10/channels/{}/messages", dm("42")),
            |b| text_of(b) == "Thinking about **this**",
        )
        .await;
    assert_eq!(first["allowed_mentions"], json!({ "parse": [] }));
    let id = first["_message_id"].as_str().unwrap().to_string();
    out.send(ToChannel::Run {
        run_id: "r1".into(),
        place: "42".into(),
        text: "Thinking about **this**: done.".into(),
        done: true,
    })
    .unwrap();
    fake.wait_call(
        "PATCH",
        &format!("/api/v10/channels/{}/messages/{id}", dm("42")),
        |b| text_of(b) == "Thinking about **this**: done.",
    )
    .await;
    while let Ok(m) = bot.rx.try_recv() {
        assert!(
            !matches!(&m, FromChannel::Message { text, .. } if text == "hello server"),
            "{m:?}"
        );
    }

    // Removing her: she's told, and her messages are ignored from then on.
    bot.handle.remove_person("42");
    fake.wait_call(
        "POST",
        &format!("/api/v10/channels/{}/messages", dm("42")),
        |b| text_of(b).contains("any more"),
    )
    .await;
    fake.say(&alice, "still there?");
    tokio::time::sleep(Duration::from_millis(800)).await;
    while let Ok(m) = bot.rx.try_recv() {
        assert!(!matches!(m, FromChannel::Message { .. }), "{m:?}");
    }
}

#[tokio::test]
async fn approvals_have_buttons_and_questions_take_replies() {
    let fake = Fake::start().await;
    let mut bot = start_with(fake.clone(), TOKEN.as_str(), true, None).await;
    bot.connected().await;
    let alice = bot.let_alice_in().await;
    let out = bot.handle.sender();
    let messages = format!("/api/v10/channels/{}/messages", dm("42"));

    out.send(ToChannel::Question {
        place: "42".into(),
        ask_id: "ask-1".into(),
        from: "Helper".into(),
        text: "wants to use fetch".into(),
        question: false,
    })
    .unwrap();
    let q = fake
        .wait_call("POST", &messages, |b| b["components"].is_array())
        .await;
    assert!(
        text_of(&q).starts_with("**Helper:** wants to use fetch"),
        "{q}"
    );
    let buttons = &q["components"][0]["components"];
    assert_eq!(buttons[0]["label"], "Allow");
    let allow = buttons[0]["custom_id"].as_str().unwrap().to_string();
    assert!(allow.ends_with(":y"));
    let qid = q["_message_id"].as_str().unwrap().to_string();
    // Someone else's press does nothing.
    fake.press(&user("66", "eve"), &qid, &allow);
    fake.press(&alice, &qid, &allow);
    let (conv, text, answers) = bot.next_message().await;
    assert_eq!(conv.place, "42");
    assert_eq!(
        (text.as_str(), answers.as_deref()),
        ("/allow", Some("ask-1"))
    );
    let cb = fake
        .wait_call("POST", "/api/v10/interactions/", |b| b["type"] == 7)
        .await;
    assert_eq!(cb["data"]["components"], json!([]));
    assert!(
        cb["data"]["content"]
            .as_str()
            .unwrap()
            .ends_with("**Allowed.**")
    );
    // Only Alice's press was answered.
    assert_eq!(fake.calls("POST", "/api/v10/interactions/").len(), 1);

    // A question: answered by replying to it.
    out.send(ToChannel::Question {
        place: "42".into(),
        ask_id: "ask-2".into(),
        from: "Helper".into(),
        text: "Which file?".into(),
        question: true,
    })
    .unwrap();
    let q = fake
        .wait_call("POST", &messages, |b| text_of(b).contains("Which file?"))
        .await;
    assert!(text_of(&q).contains("Reply to this message"));
    let qid = q["_message_id"].as_str().unwrap().to_string();
    fake.reply(&alice, "the second one", &qid);
    let (_, text, answers) = bot.next_message().await;
    assert_eq!(
        (text.as_str(), answers.as_deref()),
        ("the second one", Some("ask-2"))
    );

    // Closed: the buttons go; one pressed later is told it doesn't wait.
    out.send(ToChannel::Closed {
        ask_id: "ask-1".into(),
    })
    .unwrap();
    fake.wait_call("PATCH", &messages, |b| b["components"] == json!([]))
        .await;
    fake.press(&alice, "1", &allow);
    fake.wait_call("POST", "/api/v10/interactions/", |b| {
        b["data"]["content"]
            .as_str()
            .unwrap_or("")
            .contains("doesn't wait any more")
    })
    .await;
}

#[tokio::test]
async fn without_streaming_it_comes_at_the_end_and_long_answers_are_split() {
    let fake = Fake::start().await;
    let mut bot = start_with(fake.clone(), TOKEN.as_str(), false, None).await;
    bot.connected().await;
    bot.let_alice_in().await;
    let out = bot.handle.sender();
    let run = |text: String, done: bool| ToChannel::Run {
        run_id: "r".into(),
        place: "42".into(),
        text,
        done,
    };
    out.send(run("Working".into(), false)).unwrap();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(!fake.sent_to("42").iter().any(|t| t == "Working"));
    let long = format!("{}\n\n{}", "a".repeat(1500), "b".repeat(1500));
    out.send(run(long, true)).unwrap();
    fake.wait_call(
        "POST",
        &format!("/api/v10/channels/{}/messages", dm("42")),
        |b| text_of(b).starts_with("bbb"),
    )
    .await;
    let parts: Vec<String> = fake
        .sent_to("42")
        .into_iter()
        .filter(|t| t.starts_with("aaa") || t.starts_with("bbb"))
        .collect();
    assert_eq!(parts.len(), 2, "{parts:?}");
    assert!(parts.iter().all(|p| p.len() <= 2000));
}

#[tokio::test]
async fn files_are_saved_and_old_messages_get_a_note() {
    let fake = Fake::start().await;
    let mut bot = start_with(fake.clone(), TOKEN.as_str(), true, None).await;
    bot.connected().await;
    let alice = bot.let_alice_in().await;
    let messages = format!("/api/v10/channels/{}/messages", dm("42"));

    fake.send_file(&alice, "notes.txt", b"hello");
    fake.wait_call("POST", &messages, |b| {
        text_of(b).contains("Saved to uploads/notes.txt")
    })
    .await;
    assert_eq!(
        bot.seen.lock().unwrap().saved,
        vec![("notes.txt".to_string(), b"hello".to_vec())]
    );
    // Over the channel's 1 MB.
    fake.send_file(&alice, "big.bin", &vec![0u8; 1024 * 1024 + 1]);
    fake.wait_call("POST", &messages, |b| {
        text_of(b).contains("big.bin") && text_of(b).contains("1 MB at most")
    })
    .await;

    fake.say_at(&alice, "from long ago", "2001-09-09T01:46:40Z");
    fake.wait_call("POST", &messages, |b| {
        text_of(b).contains("came while Chummy was off")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(m) = bot.rx.try_recv() {
        assert!(!matches!(m, FromChannel::Message { .. }), "{m:?}");
    }
}

#[tokio::test]
async fn it_resumes_after_a_drop_and_catches_up_after_being_off() {
    let fake = Fake::start().await;
    let mut bot = start_with(fake.clone(), TOKEN.as_str(), true, None).await;
    bot.connected().await;
    let alice = bot.let_alice_in().await;
    assert_eq!(fake.identifies(), 1);

    // The connection drops; what's said meanwhile comes when it resumes.
    fake.drop_gateway(4000);
    fake.say(&alice, "while you were away");
    let (_, text, _) = bot.next_message().await;
    assert_eq!(text, "while you were away");
    assert_eq!((fake.identifies(), fake.resumes()), (1, 1));

    // A session that can't be resumed: it identifies again.
    fake.drop_gateway(4009);
    until("identified again", || {
        (fake.identifies() == 2).then_some(())
    })
    .await;
    fake.say(&alice, "still here");
    assert_eq!(bot.next_message().await.1, "still here");

    // Off: what Alice says is fetched when it's back on.
    let kept = bot.host.kept();
    bot.handle.stop();
    drop(bot);
    tokio::time::sleep(Duration::from_millis(300)).await;
    fake.say(&alice, "said while you were off");
    let mut again = start_with(fake.clone(), TOKEN.as_str(), true, kept).await;
    let (conv, text, _) = again.next_message().await;
    assert_eq!(
        (conv.place.as_str(), text.as_str()),
        ("42", "said while you were off")
    );
}

#[tokio::test]
async fn what_goes_wrong_is_said_plainly_without_the_token() {
    // A token Discord doesn't know.
    let fake = Fake::start().await;
    let other = chummy_channel::discord::fake_token("987654321098765432");
    let bot = start_with(fake.clone(), &other, true, None).await;
    let s = until("a problem", || {
        bot.seen
            .lock()
            .unwrap()
            .statuses
            .iter()
            .find_map(|s| match s {
                Status::Problem(m) => Some(m.clone()),
                _ => None,
            })
    })
    .await;
    assert!(s.contains("doesn't accept the bot's token"), "{s}");
    assert!(!s.contains(&other) && !s.contains("fakefake"), "{s}");
    drop(bot);

    // A public bot in no server: both are pointed out, with the link that
    // adds it to one.
    let fake = Fake::start().await;
    fake.set_bot_public(true);
    fake.set_servers(0);
    let bot = start_with(fake.clone(), TOKEN.as_str(), true, None).await;
    bot.connected().await;
    let notes = until("the notes", || {
        let n = bot.notes();
        (n.len() >= 2).then_some(n)
    })
    .await;
    assert!(notes.iter().any(|n| n.contains("Public Bot")), "{notes:?}");
    assert!(
        notes.iter().any(|n| n.contains(
            "https://discord.com/oauth2/authorize?client_id=123456789012345678&scope=bot"
        )),
        "{notes:?}"
    );
}

#[tokio::test]
async fn people_let_in_stay_across_restarts_but_not_for_another_bot() {
    let fake = Fake::start().await;
    let mut bot = start_with(fake.clone(), TOKEN.as_str(), true, None).await;
    bot.connected().await;
    bot.let_alice_in().await;
    let kept = bot.host.kept();
    bot.handle.stop();
    drop(bot);

    let mut again = start_with(fake.clone(), TOKEN.as_str(), true, kept.clone()).await;
    loop {
        if let FromChannel::Ready { convs } = again.next().await {
            assert_eq!(convs.len(), 1);
            break;
        }
    }
    drop(again);

    // What was kept says another bot: nobody is let in.
    let mut k = kept.unwrap();
    k["bot_id"] = json!("5");
    let mut other = start_with(fake, TOKEN.as_str(), true, Some(k)).await;
    loop {
        if let FromChannel::Ready { convs } = other.next().await {
            assert!(convs.is_empty());
            break;
        }
    }
}
