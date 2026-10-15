//! Chats with the agent about a recording, stored with its transcript: questions, answers and
//! each answer's details (the steps the agent took), kept whole.

use crate::transcript::Transcript;
use rusqlite::{params, OptionalExtension};
use std::time::{SystemTime, UNIX_EPOCH};

pub type ChatId = i64;

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn memory_key(chat: ChatId) -> String {
    format!("chat-memory:{chat}")
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatInfo {
    pub id: ChatId,
    pub title: String,
    pub messages: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Author {
    User,
    Agent,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub author: Author,
    pub text: String,
    /// The agent's steps (JSON), `Null` for the user's messages.
    pub details: serde_json::Value,
}

impl Transcript {
    pub fn create_chat(&self, title: &str) -> anyhow::Result<ChatId> {
        self.db.execute("INSERT INTO chats (title, created_ms) VALUES (?1, ?2)", params![title, now_ms()])?;
        Ok(self.db.last_insert_rowid())
    }

    /// Chats, the newest first.
    pub fn chats(&self) -> anyhow::Result<Vec<ChatInfo>> {
        let mut q = self.db.prepare("SELECT c.id, c.title, (SELECT COUNT(*) FROM messages m WHERE m.chat = c.id) FROM chats c ORDER BY c.id DESC")?;
        let rows = q.query_map([], |r| Ok(ChatInfo { id: r.get(0)?, title: r.get(1)?, messages: r.get::<_, i64>(2)? as usize }))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn chat_title(&self, chat: ChatId) -> anyhow::Result<Option<String>> {
        Ok(self.db.query_row("SELECT title FROM chats WHERE id = ?1", [chat], |r| r.get(0)).optional()?)
    }

    pub fn rename_chat(&self, chat: ChatId, title: &str) -> anyhow::Result<()> {
        self.db.execute("UPDATE chats SET title = ?1 WHERE id = ?2", params![title.trim(), chat])?;
        Ok(())
    }

    /// Removes a chat with its messages and memory.
    pub fn delete_chat(&self, chat: ChatId) -> anyhow::Result<()> {
        self.db.execute("DELETE FROM chats WHERE id = ?1", [chat])?;
        self.db.execute("DELETE FROM meta WHERE key = ?1", [memory_key(chat)])?;
        Ok(())
    }

    /// What the agent keeps for a chat (opaque JSON: remembered facts, the summary).
    pub fn chat_memory(&self, chat: ChatId) -> anyhow::Result<Option<String>> {
        self.meta(&memory_key(chat))
    }

    pub fn set_chat_memory(&self, chat: ChatId, memory: &str) -> anyhow::Result<()> {
        self.set_meta(&memory_key(chat), memory)
    }

    pub fn add_message(&self, chat: ChatId, author: Author, text: &str, details: &serde_json::Value) -> anyhow::Result<()> {
        let role = match author {
            Author::User => "user",
            Author::Agent => "agent",
        };
        self.db.execute(
            "INSERT INTO messages (chat, role, text, details, created_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![chat, role, text, details.to_string(), now_ms()],
        )?;
        Ok(())
    }

    pub fn messages(&self, chat: ChatId) -> anyhow::Result<Vec<ChatMessage>> {
        let mut q = self.db.prepare("SELECT role, text, details FROM messages WHERE chat = ?1 ORDER BY id")?;
        let rows = q.query_map([chat], |r| {
            let role: String = r.get(0)?;
            let details: String = r.get(2)?;
            Ok(ChatMessage {
                author: if role == "user" { Author::User } else { Author::Agent },
                text: r.get(1)?,
                details: serde_json::from_str(&details).unwrap_or(serde_json::Value::Null),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chats_keep_their_messages() {
        let dir = tempfile::tempdir().unwrap();
        let t = Transcript::open(dir.path(), "f").unwrap();
        let a = t.create_chat("Сроки").unwrap();
        let b = t.create_chat("Бюджет").unwrap();
        t.add_message(a, Author::User, "Когда отчёт?", &serde_json::Value::Null).unwrap();
        t.add_message(a, Author::Agent, "В пятницу [0:10].", &json!([{"tool": "search"}])).unwrap();
        let chats = t.chats().unwrap();
        assert_eq!(chats.iter().map(|c| (c.title.as_str(), c.messages)).collect::<Vec<_>>(), [("Бюджет", 0), ("Сроки", 2)], "newest first");
        let m = t.messages(a).unwrap();
        assert_eq!((m[0].author, m[1].author), (Author::User, Author::Agent));
        assert_eq!(m[1].details[0]["tool"], "search");
        t.set_chat_memory(a, r#"{"facts":["x"]}"#).unwrap();
        assert_eq!(t.chat_memory(a).unwrap().as_deref(), Some(r#"{"facts":["x"]}"#));
        assert_eq!(t.chat_memory(b).unwrap(), None);
        t.rename_chat(b, " Деньги ").unwrap();
        assert_eq!(t.chat_title(b).unwrap().as_deref(), Some("Деньги"));
        t.delete_chat(a).unwrap();
        assert!(t.messages(a).unwrap().is_empty(), "messages go with the chat");
        assert_eq!(t.chat_memory(a).unwrap(), None, "and its memory");
        assert_eq!(t.chats().unwrap().len(), 1);
        assert_eq!(t.chat_title(a).unwrap(), None);
    }
}
