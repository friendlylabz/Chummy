//! The Telegram channel against a stand-in Bot API: pairing, talking,
//! answers shown as they're written, approvals with buttons, files, groups,
//! and what goes wrong.

#![cfg(feature = "telegram")]

#[path = "support/fake_telegram.rs"]
mod fake;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chummy_channel::{
    ChannelHost, Event, FromChannel, Handle, Options, Secrets, Start, Status, ToChannel, Verify,
};
use fake::{Fake, TOKEN, user};
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

/// A Telegram channel as the programs start one: through its kind.
fn settings(fake: &Fake, token: &str, stream: bool) -> Start {
    let mut options = Options::new();
    options.insert("api_url".into(), fake.url.clone().into());
    options.insert("upload_mb".into(), 1.into());
    options.insert("stream".into(), stream.into());
    let mut secrets = Secrets::new();
    secrets.insert("token".into(), zeroize::Zeroizing::new(token.into()));
    Start {
        channel: "ch-t".into(),
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
    let kind = chummy_channel::kind("telegram").unwrap();
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

    /// Lets Alice (id 42) in through a pairing code.
    async fn pair_alice(&mut self) -> Value {
        let alice = user(42, "Alice", "alice");
        let code = self.handle.invite().await.unwrap();
        assert!(
            code.text.contains(&format!("/start {}", code.code)),
            "{}",
            code.text
        );
        self.fake.say(&alice, &format!("/start {}", code.code));
        let n = until("the pairing", || {
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
            .wait_call("sendMessage", |b| {
                b["chat_id"] == json!(42)
                    && b["text"].as_str().unwrap().contains("can talk to Helper")
            })
            .await;
        alice
    }
}

#[tokio::test]
async fn people_pair_with_a_code_and_then_talk() {
    let fake = Fake::start().await;
    let mut bot = start_with(fake.clone(), TOKEN, true, None).await;
    bot.connected().await;
    assert!(
        bot.event(|e| *e
            == Event::Account {
                name: "@helper_bot".into()
            })
        .is_some()
    );
    assert!(!fake.calls("setMyCommands").is_empty());

    // Strangers get nothing: not a hello, not a wrong code.
    let eve = user(66, "Eve", "eve");
    fake.say(&eve, "hello");
    let code = bot.handle.invite().await.unwrap();
    assert_eq!(code.code.len(), 6);
    assert!(code.text.contains("@helper_bot"), "{}", code.text);
    let wrong = if code.code == "000000" {
        "111111"
    } else {
        "000000"
    };
    fake.say(&eve, &format!("/start {wrong}"));
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(fake.sent_to(66).is_empty(), "{:?}", fake.sent_to(66));
    assert!(bot.event(|e| verify_n(e).is_some()).is_none());

    // The right code: they ask, and wait to be let in.
    let alice = user(42, "Alice", "alice");
    fake.say(&alice, &format!("/start {}", code.code));
    let n = until("the pairing", || {
        bot.event(|e| verify_n(e).is_some())
            .and_then(|e| verify_n(&e))
            .map(|(n, _)| n)
    })
    .await;
    fake.wait_call("sendMessage", |b| {
        b["chat_id"] == json!(42)
            && b["text"]
                .as_str()
                .unwrap()
                .contains("wait for whoever runs Chummy")
    })
    .await;
    // A code works once.
    let bob = user(7, "Bob", "bob");
    fake.say(&bob, &format!("/start {}", code.code));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(fake.sent_to(7).is_empty());

    bot.handle.answer(Some(n), true);
    until("Alice let in", || {
        bot.event(|e| matches!(e, Event::People { people } if people.len() == 1))
    })
    .await;
    let kept = bot.host.kept().unwrap();
    assert_eq!(chummy_channel::telegram::kept_people(&kept)[0].id, "42");

    // Alice talks: her message is passed on, as her conversation.
    fake.say(&alice, "hi there");
    let (conv, text, answers) = bot.next_message().await;
    assert_eq!((conv.place.as_str(), conv.agent.as_str()), ("42", "helper"));
    assert_eq!(conv.chat, chummy_channel::channel_chat("ch-t", "42"));
    assert_eq!((text.as_str(), answers), ("hi there", None));
    fake.say(&alice, "/status@helper_bot");
    assert_eq!(bot.next_message().await.1, "/status");

    // The answer, as it's written: a draft, then the message.
    let out = bot.handle.sender();
    out.send(ToChannel::Started {
        run_id: "r1".into(),
        place: "42".into(),
    })
    .unwrap();
    fake.wait_call("sendChatAction", |b| b["chat_id"] == json!(42))
        .await;
    out.send(ToChannel::Run {
        run_id: "r1".into(),
        place: "42".into(),
        text: "Thinking about **this**".into(),
        done: false,
    })
    .unwrap();
    let d = fake
        .wait_call("sendMessageDraft", |b| b["chat_id"] == json!(42))
        .await;
    assert_eq!(d["text"], "Thinking about <b>this</b>");
    assert_eq!(d["parse_mode"], "HTML");
    out.send(ToChannel::Run {
        run_id: "r1".into(),
        place: "42".into(),
        text: "Thinking about **this**: done.".into(),
        done: true,
    })
    .unwrap();
    fake.wait_call("sendMessage", |b| {
        b["text"] == json!("Thinking about <b>this</b>: done.")
    })
    .await;

    // Removing her: she's told, and her messages are ignored from then on.
    bot.handle.remove_person("42");
    fake.wait_call("sendMessage", |b| {
        b["chat_id"] == json!(42) && b["text"].as_str().unwrap().contains("any more")
    })
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
    let mut bot = start_with(fake.clone(), TOKEN, true, None).await;
    bot.connected().await;
    let alice = bot.pair_alice().await;
    let out = bot.handle.sender();

    out.send(ToChannel::Question {
        place: "42".into(),
        ask_id: "ask-1".into(),
        from: "Helper".into(),
        text: "wants to use fetch".into(),
        question: false,
    })
    .unwrap();
    let q = fake
        .wait_call("sendMessage", |b| {
            b["reply_markup"]["inline_keyboard"].is_array()
        })
        .await;
    assert!(
        q["text"]
            .as_str()
            .unwrap()
            .starts_with("<b>Helper:</b> wants to use fetch")
    );
    let allow = q["reply_markup"]["inline_keyboard"][0][0]["callback_data"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(allow.ends_with(":y"));
    let qid = q["_message_id"].as_i64().unwrap();
    // Someone else's press does nothing.
    fake.press(&user(66, "Eve", "eve"), qid, &allow);
    fake.press(&alice, qid, &allow);
    let (conv, text, answers) = bot.next_message().await;
    assert_eq!(conv.place, "42");
    assert_eq!(
        (text.as_str(), answers.as_deref()),
        ("/allow", Some("ask-1"))
    );
    fake.wait_call("answerCallbackQuery", |b| b["text"] == json!("Allowed"))
        .await;
    fake.wait_call("editMessageReplyMarkup", |_| true).await;

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
        .wait_call("sendMessage", |b| {
            b["reply_markup"]["force_reply"] == json!(true)
        })
        .await;
    assert!(q["text"].as_str().unwrap().contains("Which file?"));
    let qid = q["_message_id"].as_i64().unwrap();
    fake.reply(&alice, "the second one", qid);
    let (_, text, answers) = bot.next_message().await;
    assert_eq!(
        (text.as_str(), answers.as_deref()),
        ("the second one", Some("ask-2"))
    );

    // Closed: a button pressed later is told it doesn't wait.
    out.send(ToChannel::Closed {
        ask_id: "ask-1".into(),
    })
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    fake.press(&alice, 1, &allow);
    fake.wait_call("answerCallbackQuery", |b| {
        b["text"] == json!("That doesn't wait any more.")
    })
    .await;
}

#[tokio::test]
async fn without_drafts_one_message_grows_and_without_streaming_it_comes_at_the_end() {
    let fake = Fake::start().await;
    fake.set_no_drafts(true);
    let mut bot = start_with(fake.clone(), TOKEN, true, None).await;
    bot.connected().await;
    bot.pair_alice().await;
    let out = bot.handle.sender();
    let run = |text: &str, done: bool| ToChannel::Run {
        run_id: "r".into(),
        place: "42".into(),
        text: text.into(),
        done,
    };
    out.send(run("Part one", false)).unwrap();
    fake.wait_call("sendMessage", |b| b["text"] == json!("Part one"))
        .await;
    out.send(run("Part one, part two", true)).unwrap();
    fake.wait_call("editMessageText", |b| {
        b["text"] == json!("Part one, part two")
    })
    .await;

    // Streaming off: nothing until it's done.
    let fake2 = Fake::start().await;
    let mut quiet = start_with(fake2.clone(), TOKEN, false, None).await;
    quiet.connected().await;
    quiet.pair_alice().await;
    let out = quiet.handle.sender();
    out.send(run("Working", false)).unwrap();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(fake2.calls("sendMessageDraft").is_empty());
    assert!(!fake2.sent_to(42).iter().any(|t| t == "Working"));
    out.send(run("Working: finished", true)).unwrap();
    fake2
        .wait_call("sendMessage", |b| b["text"] == json!("Working: finished"))
        .await;

    // HTML Telegram can't read goes as plain text.
    fake2.set_reject_html(true);
    out.send(ToChannel::Notice {
        place: "42".into(),
        text: "plain **please**".into(),
    })
    .unwrap();
    fake2
        .wait_call("sendMessage", |b| {
            b["text"] == json!("plain **please**") && b.get("parse_mode").is_none()
        })
        .await;
}

#[tokio::test]
async fn files_are_saved_groups_are_left_and_old_messages_get_a_note() {
    let fake = Fake::start().await;
    let mut bot = start_with(fake.clone(), TOKEN, true, None).await;
    bot.connected().await;
    let alice = bot.pair_alice().await;

    fake.send_file(&alice, "notes.txt", b"hello");
    fake.wait_call("sendMessage", |b| {
        b["text"]
            .as_str()
            .unwrap()
            .contains("Saved to uploads/notes.txt")
    })
    .await;
    assert_eq!(
        bot.seen.lock().unwrap().saved,
        vec![("notes.txt".to_string(), b"hello".to_vec())]
    );
    // Over the channel's 1 MB.
    fake.send_file(&alice, "big.bin", &vec![0u8; 1024 * 1024 + 1]);
    fake.wait_call("sendMessage", |b| {
        b["text"].as_str().unwrap().contains("big.bin")
            && b["text"].as_str().unwrap().contains("1 MB at most")
    })
    .await;

    fake.in_group(&alice, "hello everyone");
    fake.wait_call("leaveChat", |b| b["chat_id"] == json!(-1001))
        .await;

    fake.say_at(&alice, "from long ago", 1_000_000_000);
    fake.wait_call("sendMessage", |b| {
        b["text"]
            .as_str()
            .unwrap()
            .contains("came while Chummy was off")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(m) = bot.rx.try_recv() {
        assert!(!matches!(m, FromChannel::Message { .. }), "{m:?}");
    }
}

#[tokio::test]
async fn what_goes_wrong_is_said_plainly_without_the_token() {
    // A token Telegram doesn't know.
    let fake = Fake::start().await;
    let other = "987654321:ZZHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";
    let bot = start_with(fake.clone(), other, true, None).await;
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
    assert!(!s.contains(other) && !s.contains("ZZHdq"), "{s}");
    drop(bot);

    // A webhook.
    let fake = Fake::start().await;
    fake.set_webhook("https://example.org/hook");
    let bot = start_with(fake.clone(), TOKEN, true, None).await;
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
    assert!(s.contains("webhook"), "{s}");
    drop(bot);

    // Something else reading the bot's messages.
    let fake = Fake::start().await;
    let bot = start_with(fake.clone(), TOKEN, true, None).await;
    bot.connected().await;
    fake.set_conflict(true);
    let s = until("a conflict", || {
        bot.seen
            .lock()
            .unwrap()
            .statuses
            .iter()
            .find_map(|s| match s {
                Status::Problem(m) if m.contains("Another program") => Some(m.clone()),
                _ => None,
            })
    })
    .await;
    assert!(!s.contains(TOKEN));
    // And when it's gone, it's fine again.
    fake.set_conflict(false);
    until("connected again", || {
        let st = &bot.seen.lock().unwrap().statuses;
        (st.last() == Some(&Status::Connected)).then_some(())
    })
    .await;
}

#[tokio::test]
async fn people_let_in_stay_across_restarts_but_not_for_another_bot() {
    let fake = Fake::start().await;
    let mut bot = start_with(fake.clone(), TOKEN, true, None).await;
    bot.connected().await;
    bot.pair_alice().await;
    let kept = bot.host.kept();
    bot.handle.stop();
    drop(bot);

    let mut again = start_with(fake.clone(), TOKEN, true, kept.clone()).await;
    loop {
        if let FromChannel::Ready { convs } = again.next().await {
            assert_eq!(convs.len(), 1);
            break;
        }
    }
    drop(again);

    // What was kept says another bot: nobody is let in.
    let mut k = kept.unwrap();
    k["bot_id"] = json!(5);
    let mut other = start_with(fake, TOKEN, true, Some(k)).await;
    loop {
        if let FromChannel::Ready { convs } = other.next().await {
            assert!(convs.is_empty());
            break;
        }
    }
}
