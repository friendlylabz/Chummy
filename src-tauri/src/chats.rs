//! Chats: each agent has any number of them, saved as they happen.
//!
//! On disk, in the Chummy folder's `chats/`:
//! - `<agent id>/index.json`: the chats (id, title, times, message count)
//!   and which one is open;
//! - `<agent id>/<chat id>.json`: one chat's messages.
//!
//! Before pass 12 an agent had one chat, `chats/<agent id>.json`; it's moved
//! into the folder as the agent's first chat the first time it's read.
//!
//! Every read and write goes through one lock, so runs in different chats
//! (which may finish at the same moment) never lose each other's updates to
//! the index.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chummy_store::JsonDoc;
use serde::{Deserialize, Serialize};

/// Chat history kept per chat; older messages are dropped.
pub const MAX_CHAT_MESSAGES: usize = 2000;

/// A chat's title until its first message gives it one.
pub const NEW_CHAT: &str = "New chat";

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunSummary {
    /// The run's log (Logs window, and the step view under the message).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    pub status: String,
    pub steps: u32,
    pub llm_calls: u32,
    #[serde(default)]
    pub tool_calls: u32,
    /// How long the run took, start to finish, in milliseconds (0: not
    /// recorded, for replies saved before this was added).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ms: u64,
    /// Estimated tokens this turn added to context.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tokens: u64,
    /// Estimated tokens in the context at the end, and the model's context
    /// length (0: unknown).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub context_tokens: u64,
    #[serde(default, skip_serializing_if = "is_zero32")]
    pub context_window: u32,
    /// The model's tokens used today when the reply came, and today's
    /// allowance (`None`: no daily limit).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub daily_used: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_allowed: Option<u64>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn is_zero32(n: &u32) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub id: String,
    /// "user", "assistant", "notice" (errors, stops, limits), or
    /// "question" / "answer" (something a run asked, and the reply). Only
    /// "user" and "assistant" messages are the conversation the loop sees.
    pub role: String,
    pub text: String,
    pub at: u64,
    /// Who asked a question when it wasn't this chat (a job's name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatLog {
    pub version: u32,
    pub messages: Vec<ChatMessage>,
}

/// One chat, as listed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMeta {
    pub id: String,
    pub title: String,
    /// Unix seconds.
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub messages: u32,
}

/// An agent's chats, newest first, and the open one.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ChatIndex {
    pub version: u32,
    pub active: Option<String>,
    pub chats: Vec<ChatMeta>,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn new_id() -> String {
    format!("c-{}", chummy_store::random_id(6))
}

fn valid(id: &str) -> Result<(), String> {
    if crate::library::valid_id(id) {
        Ok(())
    } else {
        Err("Invalid id.".into())
    }
}

fn agent_dir(root: &Path, agent: &str) -> Result<PathBuf, String> {
    valid(agent)?;
    Ok(root.join(agent))
}

fn index_doc(root: &Path, agent: &str) -> Result<JsonDoc<ChatIndex>, String> {
    Ok(JsonDoc::new(agent_dir(root, agent)?.join("index.json")))
}

fn chat_doc(root: &Path, agent: &str, chat: &str) -> Result<JsonDoc<ChatLog>, String> {
    valid(chat)?;
    if chat == "index" {
        return Err("Invalid id.".into());
    }
    Ok(JsonDoc::new(
        agent_dir(root, agent)?.join(format!("{chat}.json")),
    ))
}

/// A title from a chat's first message: its first line, shortened.
pub fn title_from(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut t: String = line.chars().take(48).collect();
    if line.chars().count() > 48 {
        t = t.trim_end().to_string();
        t.push('…');
    }
    if t.is_empty() { NEW_CHAT.into() } else { t }
}

fn load_index(root: &Path, agent: &str) -> Result<ChatIndex, String> {
    Ok(index_doc(root, agent)?
        .load(|v| v)
        .map_err(|e| e.to_string())?
        .value)
}

fn save_index(root: &Path, agent: &str, mut ix: ChatIndex) -> Result<ChatIndex, String> {
    ix.version = 1;
    ix.chats.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then(b.created_at.cmp(&a.created_at))
    });
    std::fs::create_dir_all(agent_dir(root, agent)?).map_err(|e| e.to_string())?;
    index_doc(root, agent)?
        .save(&ix)
        .map_err(|e| e.to_string())?;
    Ok(ix)
}

fn blank(id: String) -> ChatMeta {
    let t = now();
    ChatMeta {
        id,
        title: NEW_CHAT.into(),
        created_at: t,
        updated_at: t,
        messages: 0,
    }
}

/// Moves a chat from before pass 12 (`chats/<agent>.json`) into the folder.
fn migrate_legacy(root: &Path, agent: &str, ix: &mut ChatIndex) -> Result<bool, String> {
    let legacy = root.join(format!("{agent}.json"));
    if !legacy.exists() {
        return Ok(false);
    }
    let log = JsonDoc::<ChatLog>::new(&legacy)
        .load(|v| v)
        .map_err(|e| e.to_string())?
        .value;
    let id = new_id();
    let doc = chat_doc(root, agent, &id)?;
    std::fs::create_dir_all(agent_dir(root, agent)?).map_err(|e| e.to_string())?;
    doc.save(&log).map_err(|e| e.to_string())?;
    let first = log.messages.iter().find(|m| m.role == "user");
    let last = log.messages.last().map(|m| m.at).unwrap_or_else(now);
    ix.chats.push(ChatMeta {
        id: id.clone(),
        title: first.map_or_else(|| NEW_CHAT.to_string(), |m| title_from(&m.text)),
        created_at: log.messages.first().map(|m| m.at).unwrap_or(last),
        updated_at: last,
        messages: log.messages.len() as u32,
    });
    ix.active.get_or_insert(id);
    let _ = std::fs::remove_file(&legacy);
    let mut bak = legacy.into_os_string();
    bak.push(".bak");
    let _ = std::fs::remove_file(bak);
    Ok(true)
}

/// An agent's chats. There's always at least one (a new, empty chat is made
/// if there are none), and `active` always names one of them.
pub fn index(root: &Path, agent: &str) -> Result<ChatIndex, String> {
    let _g = lock();
    index_locked(root, agent)
}

fn index_locked(root: &Path, agent: &str) -> Result<ChatIndex, String> {
    let mut ix = load_index(root, agent)?;
    let mut changed = migrate_legacy(root, agent, &mut ix)?;
    if ix.chats.is_empty() {
        let c = blank(new_id());
        ix.active = Some(c.id.clone());
        ix.chats.push(c);
        changed = true;
    }
    if !ix
        .active
        .as_ref()
        .is_some_and(|a| ix.chats.iter().any(|c| &c.id == a))
    {
        ix.active = ix
            .chats
            .iter()
            .max_by_key(|c| c.updated_at)
            .map(|c| c.id.clone());
        changed = true;
    }
    if changed {
        ix = save_index(root, agent, ix)?;
    }
    Ok(ix)
}

/// Makes a new, empty chat and opens it.
pub fn create(root: &Path, agent: &str) -> Result<(ChatMeta, ChatIndex), String> {
    let _g = lock();
    let mut ix = index_locked(root, agent)?;
    let c = blank(new_id());
    ix.active = Some(c.id.clone());
    ix.chats.push(c.clone());
    Ok((c, save_index(root, agent, ix)?))
}

/// Opens a chat.
pub fn set_active(root: &Path, agent: &str, chat: &str) -> Result<ChatIndex, String> {
    let _g = lock();
    let mut ix = index_locked(root, agent)?;
    if !ix.chats.iter().any(|c| c.id == chat) {
        return Err("That chat no longer exists.".into());
    }
    ix.active = Some(chat.to_string());
    save_index(root, agent, ix)
}

/// A chat's messages.
pub fn load(root: &Path, agent: &str, chat: &str) -> Result<Vec<ChatMessage>, String> {
    let _g = lock();
    Ok(chat_doc(root, agent, chat)?
        .load(|v| v)
        .map_err(|e| e.to_string())?
        .value
        .messages)
}

/// Adds a message to a chat (dropping the oldest past the limit) and
/// updates the chat's title, time and count. Returns the updated index.
pub fn append(
    root: &Path,
    agent: &str,
    chat: &str,
    message: &ChatMessage,
) -> Result<ChatIndex, String> {
    let _g = lock();
    let mut ix = index_locked(root, agent)?;
    let Some(meta) = ix.chats.iter_mut().find(|c| c.id == chat) else {
        return Err("That chat no longer exists.".into());
    };
    let doc = chat_doc(root, agent, chat)?;
    let mut log = doc.load(|v| v).map_err(|e| e.to_string())?.value;
    log.version = 2;
    log.messages.push(message.clone());
    if log.messages.len() > MAX_CHAT_MESSAGES {
        let extra = log.messages.len() - MAX_CHAT_MESSAGES;
        log.messages.drain(..extra);
    }
    doc.save(&log).map_err(|e| e.to_string())?;
    if meta.title == NEW_CHAT && message.role == "user" {
        meta.title = title_from(&message.text);
    }
    meta.updated_at = now();
    meta.messages = log.messages.len() as u32;
    save_index(root, agent, ix)
}

/// Deletes one message from a chat, so it's gone from the history the loop
/// sees too. Returns the updated index, the message and where it was, so it
/// can be put back (`restore_message`).
pub fn delete_message(
    root: &Path,
    agent: &str,
    chat: &str,
    message: &str,
) -> Result<(ChatIndex, ChatMessage, usize), String> {
    let _g = lock();
    let mut ix = index_locked(root, agent)?;
    let Some(meta) = ix.chats.iter_mut().find(|c| c.id == chat) else {
        return Err("That chat no longer exists.".into());
    };
    let doc = chat_doc(root, agent, chat)?;
    let mut log = doc.load(|v| v).map_err(|e| e.to_string())?.value;
    let Some(pos) = log.messages.iter().position(|m| m.id == message) else {
        return Err("That message is already gone.".into());
    };
    let removed = log.messages.remove(pos);
    log.version = 2;
    doc.save(&log).map_err(|e| e.to_string())?;
    meta.messages = log.messages.len() as u32;
    Ok((save_index(root, agent, ix)?, removed, pos))
}

/// Puts a deleted message back where it was (undo). `position` is clamped to
/// the chat's length. Returns the updated index and the position used.
pub fn restore_message(
    root: &Path,
    agent: &str,
    chat: &str,
    message: ChatMessage,
    position: usize,
) -> Result<(ChatIndex, usize), String> {
    let _g = lock();
    let mut ix = index_locked(root, agent)?;
    let Some(meta) = ix.chats.iter_mut().find(|c| c.id == chat) else {
        return Err("That chat no longer exists.".into());
    };
    let doc = chat_doc(root, agent, chat)?;
    let mut log = doc.load(|v| v).map_err(|e| e.to_string())?.value;
    if log.messages.iter().any(|m| m.id == message.id) {
        return Err("That message is already back.".into());
    }
    let pos = position.min(log.messages.len());
    log.messages.insert(pos, message);
    log.version = 2;
    doc.save(&log).map_err(|e| e.to_string())?;
    meta.messages = log.messages.len() as u32;
    Ok((save_index(root, agent, ix)?, pos))
}

/// Renames a chat. Returns the updated index.
pub fn rename(root: &Path, agent: &str, chat: &str, title: &str) -> Result<ChatIndex, String> {
    let _g = lock();
    let mut ix = index_locked(root, agent)?;
    let Some(meta) = ix.chats.iter_mut().find(|c| c.id == chat) else {
        return Err("That chat no longer exists.".into());
    };
    meta.title = title_from(title);
    save_index(root, agent, ix)
}

/// Deletes a chat. If it was the last one, a new empty chat takes its place.
pub fn delete(root: &Path, agent: &str, chat: &str) -> Result<ChatIndex, String> {
    let _g = lock();
    let mut ix = index_locked(root, agent)?;
    let doc = chat_doc(root, agent, chat)?;
    ix.chats.retain(|c| c.id != chat);
    let path = doc.path().to_path_buf();
    let _ = std::fs::remove_file(&path);
    let mut bak = path.into_os_string();
    bak.push(".bak");
    let _ = std::fs::remove_file(bak);
    save_index(root, agent, ix)?;
    // Makes a new chat if that was the last one, and picks the open one.
    index_locked(root, agent)
}

/// Removes every chat of an agent (when the agent is deleted).
pub fn delete_agent(root: &Path, agent: &str) -> Result<(), String> {
    let _g = lock();
    let dir = agent_dir(root, agent)?;
    let _ = std::fs::remove_dir_all(dir);
    for name in [format!("{agent}.json"), format!("{agent}.json.bak")] {
        let _ = std::fs::remove_file(root.join(name));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, text: &str) -> ChatMessage {
        ChatMessage {
            id: new_id(),
            role: role.into(),
            text: text.into(),
            at: 5,
            from: None,
            run: None,
        }
    }

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("chummy-chats-{}", new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn an_agent_always_has_a_chat_and_new_ones_open() {
        let root = tmp();
        let ix = index(&root, "a1").unwrap();
        assert_eq!(ix.chats.len(), 1);
        assert_eq!(ix.chats[0].title, NEW_CHAT);
        assert_eq!(ix.active.as_deref(), Some(ix.chats[0].id.as_str()));
        // Reading again doesn't make another.
        assert_eq!(index(&root, "a1").unwrap(), ix);
        let (c, ix2) = create(&root, "a1").unwrap();
        assert_eq!(ix2.chats.len(), 2);
        assert_eq!(ix2.active.as_deref(), Some(c.id.as_str()));
        set_active(&root, "a1", &ix.chats[0].id).unwrap();
        assert_eq!(index(&root, "a1").unwrap().active, ix.active);
        assert!(set_active(&root, "a1", "c-nope").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn messages_title_and_order() {
        let root = tmp();
        let first = index(&root, "a1").unwrap().chats[0].id.clone();
        let (second, _) = create(&root, "a1").unwrap();
        append(
            &root,
            "a1",
            &first,
            &msg("user", "\n  Plan my trip to Lisbon in May please\nmore"),
        )
        .unwrap();
        let ix = append(&root, "a1", &first, &msg("assistant", "Sure.")).unwrap();
        let c = ix.chats.iter().find(|c| c.id == first).unwrap();
        assert_eq!(c.title, "Plan my trip to Lisbon in May please");
        assert_eq!(c.messages, 2);
        assert_eq!(load(&root, "a1", &first).unwrap().len(), 2);
        assert!(load(&root, "a1", &second.id).unwrap().is_empty());
        // A later message doesn't rename it.
        let ix = append(&root, "a1", &first, &msg("user", "Another thing")).unwrap();
        assert_eq!(
            ix.chats.iter().find(|c| c.id == first).unwrap().title,
            "Plan my trip to Lisbon in May please"
        );
        assert_eq!(title_from(&"x".repeat(60)).chars().count(), 49);
        // Ids can't reach outside.
        assert!(load(&root, "a1", "../x").is_err());
        assert!(load(&root, "../a", &first).is_err());
        assert!(append(&root, "a1", "c-gone", &msg("user", "x")).is_err());
        std::fs::remove_dir_all(root).unwrap();

        // (was run_time_is_kept_and_optional)
        {
            // Replies saved before run times were recorded still load.
            let old: RunSummary =
                serde_json::from_str(r#"{"status":"completed","steps":3,"llm_calls":1}"#).unwrap();
            assert_eq!(old.ms, 0);
            assert!(serde_json::to_value(&old).unwrap().get("ms").is_none());
            let new = RunSummary { ms: 65_300, ..old };
            let v = serde_json::to_value(&new).unwrap();
            assert_eq!(v["ms"], 65_300);
            assert_eq!(serde_json::from_value::<RunSummary>(v).unwrap(), new);
        }
    }

    #[test]
    fn deleting_keeps_one_chat() {
        let root = tmp();
        let only = index(&root, "a1").unwrap().chats[0].id.clone();
        append(&root, "a1", &only, &msg("user", "hi")).unwrap();
        let ix = delete(&root, "a1", &only).unwrap();
        assert_eq!(ix.chats.len(), 1);
        assert_ne!(ix.chats[0].id, only);
        assert_eq!(ix.active.as_deref(), Some(ix.chats[0].id.as_str()));
        delete_agent(&root, "a1").unwrap();
        assert!(!root.join("a1").exists());
        std::fs::remove_dir_all(root).unwrap();

        // (was deleting_one_message)
        {
            let root = tmp();
            let c = index(&root, "a1").unwrap().chats[0].id.clone();
            let (a, b) = (msg("user", "keep me"), msg("assistant", "drop me"));
            append(&root, "a1", &c, &a).unwrap();
            append(&root, "a1", &c, &b).unwrap();
            let (ix, gone, pos) = delete_message(&root, "a1", &c, &a.id).unwrap();
            assert_eq!((ix.chats[0].messages, pos), (1, 0));
            assert_eq!(gone, a);
            assert_eq!(load(&root, "a1", &c).unwrap(), vec![b.clone()]);
            assert!(
                delete_message(&root, "a1", &c, &a.id).is_err(),
                "already gone"
            );
            assert!(delete_message(&root, "a1", "c-gone", &b.id).is_err());
            assert!(delete_message(&root, "a1", "../x", &b.id).is_err());
            // Undo puts it back where it was, once.
            let (ix, pos) = restore_message(&root, "a1", &c, gone.clone(), 0).unwrap();
            assert_eq!((ix.chats[0].messages, pos), (2, 0));
            assert_eq!(load(&root, "a1", &c).unwrap(), vec![a.clone(), b.clone()]);
            assert!(
                restore_message(&root, "a1", &c, gone, 0).is_err(),
                "already back"
            );
            // A position past the end is clamped.
            let (_, gone, _) = delete_message(&root, "a1", &c, &b.id).unwrap();
            let (_, pos) = restore_message(&root, "a1", &c, gone, 99).unwrap();
            assert_eq!(pos, 1);
            assert_eq!(load(&root, "a1", &c).unwrap(), vec![a, b]);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn the_old_single_chat_becomes_the_first() {
        let root = tmp();
        let old = ChatLog {
            version: 1,
            messages: vec![msg("user", "Hello there"), msg("assistant", "Hi.")],
        };
        JsonDoc::new(root.join("a1.json")).save(&old).unwrap();
        let ix = index(&root, "a1").unwrap();
        assert_eq!(ix.chats.len(), 1);
        assert_eq!(ix.chats[0].title, "Hello there");
        assert_eq!(ix.chats[0].messages, 2);
        assert_eq!(load(&root, "a1", &ix.chats[0].id).unwrap(), old.messages);
        assert!(!root.join("a1.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
