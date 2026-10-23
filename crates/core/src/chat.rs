//! Chats with the agent about a recording, stored with its transcript: questions, answers and
//! each answer's details (the steps the agent took), kept whole.

use crate::transcript::Transcript;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub type ChatId = i64;

/// The `meta` key with the chats shown as tabs: a JSON array of ids, in tab order.
const OPEN_CHATS: &str = "chat-tabs";

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
    /// When it was started, Unix milliseconds.
    pub created_ms: i64,
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
    /// The agent's steps and timeline (JSON, see [`Timeline`]), `Null` for the user's messages.
    pub details: serde_json::Value,
}

impl ChatMessage {
    /// What happened while the answer was written (empty for the user's messages and for
    /// answers stored before timelines).
    pub fn timeline(&self) -> Timeline {
        Timeline::from_details(&self.details)
    }

    /// The answer was stopped by the user before it was finished.
    pub fn stopped(&self) -> bool {
        self.details["stopped"].as_bool() == Some(true)
    }
}

/// How a step of the timeline went (`running` until it is known).
pub const STEP_RUNNING: &str = "running";
/// A step that was still running when the answer was stopped.
pub const STEP_STOPPED: &str = "stopped";

/// One entry of an answer's timeline, in the order it happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimelineEntry {
    /// A tool call: the title the model gave it, the tool and how it went (`running`, `found`,
    /// `nothing`, `repeated`, `failed` or `stopped`). The arguments and the result are in the
    /// message's `steps`.
    Step { title: String, tool: String, status: String },
    /// The model's reasoning.
    Thinking { text: String },
    /// A part of the answer; `section`: it was also stored as a message of its own.
    Answer {
        text: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        section: bool,
    },
    /// A correction of the user taken into account in the middle of the answer.
    Steer { text: String },
}

/// The steps, reasoning and parts of the answer of one question, in order: a timeline growing
/// downward. A piece of reasoning or of the answer continues the entry before it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Timeline {
    pub entries: Vec<TimelineEntry>,
}

impl Timeline {
    /// The timeline in a message's details (`details["timeline"]`).
    pub fn from_details(details: &serde_json::Value) -> Timeline {
        let entries = serde_json::from_value(details["timeline"].clone()).unwrap_or_default();
        Timeline { entries }
    }

    /// The entries as JSON, for `details["timeline"]`.
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(&self.entries).unwrap_or(serde_json::Value::Null)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A tool call begins (`status` is not known yet) or is known already.
    pub fn step(&mut self, title: &str, tool: &str, status: &str) {
        self.entries.push(TimelineEntry::Step { title: title.to_string(), tool: tool.to_string(), status: status.to_string() });
    }

    /// The `index`-th tool step (thinking and answer entries do not count) gets `title`.
    /// `false` when there is no such step.
    pub fn set_step_title(&mut self, index: usize, title: &str) -> bool {
        let Some(slot) = self.entries.iter_mut().filter_map(|entry| match entry {
            TimelineEntry::Step { title, .. } => Some(title),
            _ => None,
        }).nth(index) else {
            return false;
        };
        *slot = title.to_string();
        true
    }

    /// A tool call is over. The calls of one step run at once and their results are shown in
    /// order: the first one still running (in the order they began) is the one that ends, and
    /// only when none is running, the last one.
    pub fn finish_step(&mut self, status: &str) {
        let running = self.entries.iter().position(|e| matches!(e, TimelineEntry::Step { status, .. } if status == STEP_RUNNING));
        let last = self.entries.iter().rposition(|e| matches!(e, TimelineEntry::Step { .. }));
        let Some(i) = running.or(last) else { return };
        if let TimelineEntry::Step { status: s, .. } = &mut self.entries[i] {
            *s = status.to_string();
        }
    }

    /// More reasoning: it continues the entry before when that is reasoning too.
    pub fn thinking(&mut self, piece: &str) {
        match self.entries.last_mut() {
            Some(TimelineEntry::Thinking { text }) => text.push_str(piece),
            _ => self.entries.push(TimelineEntry::Thinking { text: piece.to_string() }),
        }
    }

    /// More of the answer: it continues the entry before when that is a part of the answer.
    pub fn answer(&mut self, piece: &str) {
        match self.entries.last_mut() {
            Some(TimelineEntry::Answer { text, section: false }) => text.push_str(piece),
            _ => self.entries.push(TimelineEntry::Answer { text: piece.to_string(), section: false }),
        }
    }

    /// The part of the answer being written is a message of its own now (`text`, the cleaned
    /// version of what was streamed).
    pub fn section(&mut self, text: &str) {
        self.discard_answer();
        self.entries.push(TimelineEntry::Answer { text: text.to_string(), section: true });
    }

    /// The answer takes a correction of the user from here on.
    pub fn steer(&mut self, text: &str) {
        self.entries.push(TimelineEntry::Steer { text: text.to_string() });
    }

    /// The text streamed last was not the answer after all: it is taken back.
    pub fn discard_answer(&mut self) {
        if matches!(self.entries.last(), Some(TimelineEntry::Answer { section: false, .. })) {
            self.entries.pop();
        }
    }

    /// The answer is final: its last streamed part becomes `text` (the cleaned version), or is
    /// added when nothing was streamed.
    pub fn finish_answer(&mut self, text: &str) {
        if text.trim().is_empty() {
            self.discard_answer();
            return;
        }
        match self.entries.last_mut() {
            Some(TimelineEntry::Answer { text: t, section: false }) => *t = text.to_string(),
            _ => self.entries.push(TimelineEntry::Answer { text: text.to_string(), section: false }),
        }
    }

    /// The answer was stopped: steps still running are marked so.
    pub fn stop(&mut self) {
        for e in &mut self.entries {
            if let TimelineEntry::Step { status, .. } = e {
                if status == STEP_RUNNING {
                    *status = STEP_STOPPED.to_string();
                }
            }
        }
    }

    /// The part of the answer after the last stored section: what is being written.
    pub fn draft(&self) -> &str {
        match self.entries.last() {
            Some(TimelineEntry::Answer { text, section: false }) => text,
            _ => "",
        }
    }
}

impl Transcript {
    pub fn create_chat(&self, title: &str) -> anyhow::Result<ChatId> {
        self.db.execute("INSERT INTO chats (title, created_ms) VALUES (?1, ?2)", params![title, now_ms()])?;
        Ok(self.db.last_insert_rowid())
    }

    /// Chats, the newest first.
    pub fn chats(&self) -> anyhow::Result<Vec<ChatInfo>> {
        let mut q = self.db.prepare(
            "SELECT c.id, c.title, c.created_ms, (SELECT COUNT(*) FROM messages m WHERE m.chat = c.id) FROM chats c ORDER BY c.id DESC",
        )?;
        let rows = q.query_map([], |r| {
            Ok(ChatInfo { id: r.get(0)?, title: r.get(1)?, created_ms: r.get(2)?, messages: r.get::<_, i64>(3)? as usize })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The chats shown as tabs, in tab order. Without the stored list every chat is open,
    /// newest first (as they were before tabs).
    pub fn open_chats(&self) -> anyhow::Result<Vec<ChatId>> {
        if let Some(json) = self.meta(OPEN_CHATS)? {
            match serde_json::from_str::<Vec<ChatId>>(&json) {
                Ok(ids) => return Ok(ids),
                Err(e) => log::warn!("chat-tabs: {e}; treating every chat as open"),
            }
        }
        Ok(self.chats()?.into_iter().map(|c| c.id).collect())
    }

    /// Stores the tabs and their order.
    pub fn set_open_chats(&self, chats: &[ChatId]) -> anyhow::Result<()> {
        self.set_meta(OPEN_CHATS, &serde_json::to_string(chats)?)
    }

    /// Chats that have no tab (closed/archived), the newest first.
    pub fn archived_chats(&self) -> anyhow::Result<Vec<ChatInfo>> {
        let open = self.open_chats()?;
        Ok(self.chats()?.into_iter().filter(|c| !open.contains(&c.id)).collect())
    }

    pub fn chat_title(&self, chat: ChatId) -> anyhow::Result<Option<String>> {
        Ok(self.db.query_row("SELECT title FROM chats WHERE id = ?1", [chat], |r| r.get(0)).optional()?)
    }

    /// Gives a chat its title, once: a chat that has a title keeps it (there is no renaming, the
    /// user cannot rename a chat). A new chat has an empty title until this is called. Returns
    /// whether the title was set.
    pub fn set_chat_title(&self, chat: ChatId, title: &str) -> anyhow::Result<bool> {
        let title = title.trim();
        if title.is_empty() {
            return Ok(false);
        }
        let changed = self.db.execute("UPDATE chats SET title = ?1 WHERE id = ?2 AND title = ''", params![title, chat])?;
        Ok(changed > 0)
    }

    /// Removes a chat with its messages, memory and tab.
    pub fn delete_chat(&self, chat: ChatId) -> anyhow::Result<()> {
        self.db.execute("DELETE FROM chats WHERE id = ?1", [chat])?;
        self.db.execute("DELETE FROM meta WHERE key = ?1", [memory_key(chat)])?;
        let open = self.open_chats()?;
        if open.contains(&chat) {
            let left: Vec<ChatId> = open.into_iter().filter(|&c| c != chat).collect();
            self.set_open_chats(&left)?;
        }
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

    /// Writes `phrase` into the `index`-th tool step of the agent message for the answer task
    /// `answer` (`details.task`). A chat with no such message, or no such step, is left as it is.
    /// Returns whether the phrase is now that step's title.
    pub fn set_step_phrase(&self, chat: ChatId, answer: i64, index: usize, phrase: &str) -> anyhow::Result<bool> {
        if phrase.is_empty() {
            return Ok(false);
        }
        let rows = {
            let mut q = self.db.prepare("SELECT id, details FROM messages WHERE chat = ?1 AND role = 'agent' ORDER BY id DESC")?;
            let mapped = q.query_map([chat], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        for (id, raw) in rows {
            let mut details: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
            if details.get("task").and_then(|v| v.as_i64()) != Some(answer) {
                continue;
            }
            let mut timeline = Timeline::from_details(&details);
            if !timeline.set_step_title(index, phrase) {
                return Ok(false);
            }
            details["timeline"] = timeline.to_value();
            if let Some(step) = details.get_mut("steps").and_then(|v| v.as_array_mut()).and_then(|steps| steps.get_mut(index)).and_then(|v| v.as_object_mut()) {
                step.insert("title".to_string(), serde_json::Value::String(phrase.to_string()));
            }
            self.db.execute("UPDATE messages SET details = ?1 WHERE id = ?2", params![details.to_string(), id])?;
            return Ok(true);
        }
        Ok(false)
    }

    /// The correction that is asked as the chat's next question stops being a correction of the
    /// answer `from`: the first `skip` corrections of that answer are left alone. Returns whether
    /// one was found.
    pub fn unsteer(&self, chat: ChatId, from: i64, mut skip: usize) -> anyhow::Result<bool> {
        let rows = {
            let mut q = self.db.prepare("SELECT id, details FROM messages WHERE chat = ?1 AND role = 'user' ORDER BY id")?;
            let mapped = q.query_map([chat], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        for (id, raw) in rows {
            let mut details: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
            if details["steer"]["task"].as_i64() != Some(from) {
                continue;
            }
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if let Some(obj) = details.as_object_mut() {
                obj.remove("steer");
            }
            self.db.execute("UPDATE messages SET details = ?1 WHERE id = ?2", params![details.to_string(), id])?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Points the corrections the user sent for the answer `from` at the answer `to`, so that the
    /// next answer takes them: the first `skip` of them were taken by `from` and are left as they
    /// are. Returns how many moved.
    pub fn retag_steers(&self, chat: ChatId, from: i64, to: i64, mut skip: usize) -> anyhow::Result<usize> {
        let rows = {
            let mut q = self.db.prepare("SELECT id, details FROM messages WHERE chat = ?1 AND role = 'user' ORDER BY id")?;
            let mapped = q.query_map([chat], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        let mut moved = 0;
        for (id, raw) in rows {
            let mut details: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
            if details["steer"]["task"].as_i64() != Some(from) {
                continue;
            }
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if let Some(steer) = details.get_mut("steer").and_then(|s| s.as_object_mut()) {
                steer.insert("task".to_string(), serde_json::json!(to));
            }
            self.db.execute("UPDATE messages SET details = ?1 WHERE id = ?2", params![details.to_string(), id])?;
            moved += 1;
        }
        Ok(moved)
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
        assert!(chats.iter().all(|c| c.created_ms > 0));
        let m = t.messages(a).unwrap();
        assert_eq!((m[0].author, m[1].author), (Author::User, Author::Agent));
        assert_eq!(m[1].details[0]["tool"], "search");
        t.set_chat_memory(a, r#"{"facts":["x"]}"#).unwrap();
        assert_eq!(t.chat_memory(a).unwrap().as_deref(), Some(r#"{"facts":["x"]}"#));
        assert_eq!(t.chat_memory(b).unwrap(), None);
        // The title is set once: an empty one is filled, a set one never changes.
        let fresh = t.create_chat("").unwrap();
        assert!(!t.set_chat_title(fresh, "  ").unwrap(), "an empty title is nothing");
        assert!(t.set_chat_title(fresh, " Деньги ").unwrap());
        assert!(!t.set_chat_title(fresh, "Другое").unwrap());
        assert!(!t.set_chat_title(b, "Другое").unwrap());
        assert_eq!(t.chat_title(fresh).unwrap().as_deref(), Some("Деньги"));
        assert_eq!(t.chat_title(b).unwrap().as_deref(), Some("Бюджет"));
        t.delete_chat(fresh).unwrap();
        t.delete_chat(a).unwrap();
        assert!(t.messages(a).unwrap().is_empty(), "messages go with the chat");
        assert_eq!(t.chat_memory(a).unwrap(), None, "and its memory");
        assert_eq!(t.chats().unwrap().len(), 1);
        assert_eq!(t.chat_title(a).unwrap(), None);
    }

    #[test]
    fn a_timeline_grows_downward_and_round_trips_through_the_details() {
        let mut tl = Timeline::default();
        tl.thinking("Надо ");
        tl.thinking("поискать.");
        tl.step("Ищу сроки", "search", STEP_RUNNING);
        tl.finish_step("found");
        tl.thinking("Нашёл.");
        tl.answer("В пят");
        tl.answer("ницу.");
        assert_eq!(tl.draft(), "В пятницу.");
        // More steps after a part of the answer continue the same timeline.
        tl.step("Проверяю", "read", STEP_RUNNING);
        assert_eq!(tl.draft(), "");
        tl.answer("И ещё.");
        tl.finish_answer("И ещё [0:10].");
        assert_eq!(
            tl.entries,
            [
                TimelineEntry::Thinking { text: "Надо поискать.".into() },
                TimelineEntry::Step { title: "Ищу сроки".into(), tool: "search".into(), status: "found".into() },
                TimelineEntry::Thinking { text: "Нашёл.".into() },
                TimelineEntry::Answer { text: "В пятницу.".into(), section: false },
                TimelineEntry::Step { title: "Проверяю".into(), tool: "read".into(), status: "running".into() },
                TimelineEntry::Answer { text: "И ещё [0:10].".into(), section: false },
            ]
        );

        let dir = tempfile::tempdir().unwrap();
        let t = Transcript::open(dir.path(), "f").unwrap();
        let chat = t.create_chat("Сроки").unwrap();
        t.add_message(chat, Author::Agent, "И ещё [0:10].", &json!({"steps": [], "timeline": tl.to_value(), "stopped": true})).unwrap();
        t.add_message(chat, Author::Agent, "Старый ответ", &json!({"steps": []})).unwrap();
        let messages = t.messages(chat).unwrap();
        assert_eq!(messages[0].timeline(), tl, "the saved details give the same timeline");
        assert!(messages[0].stopped() && !messages[1].stopped());
        assert!(messages[1].timeline().is_empty(), "an answer from before timelines has none");
    }

    #[test]
    fn sibling_steps_of_one_call_batch_end_in_their_own_order() {
        // The calls of one step begin together and are shown as siblings; their results come
        // back in the order of the list, so each ends the first call still running.
        let mut tl = Timeline::default();
        tl.step("Поиск", "search", STEP_RUNNING);
        tl.step("Чтение записи", "read", STEP_RUNNING);
        assert_eq!(tl.draft(), "");
        tl.finish_step("found");
        tl.finish_step("nothing");
        assert_eq!(
            tl.entries,
            [
                TimelineEntry::Step { title: "Поиск".into(), tool: "search".into(), status: "found".into() },
                TimelineEntry::Step { title: "Чтение записи".into(), tool: "read".into(), status: "nothing".into() },
            ]
        );
        // A step of its own, then a result with nothing running: the last step is marked.
        tl.step("Спикеры", "speakers", STEP_RUNNING);
        tl.finish_step("found");
        tl.finish_step("failed");
        assert_eq!(tl.entries.last(), Some(&TimelineEntry::Step { title: "Спикеры".into(), tool: "speakers".into(), status: "failed".into() }));
    }

    #[test]
    fn a_step_phrase_stays_in_the_stored_timeline() {
        let dir = tempfile::tempdir().unwrap();
        let t = Transcript::open(dir.path(), "f").unwrap();
        let chat = t.create_chat("Сроки").unwrap();
        let mut tl = Timeline::default();
        tl.thinking("хм");
        tl.step("Поиск", "search", "found");
        tl.step("Чтение записи", "read", "found");
        t.add_message(
            chat,
            Author::Agent,
            "В пятницу",
            &json!({"task": 7, "steps": [{"title": "Поиск"}, {"title": "Чтение записи"}], "timeline": tl.to_value()}),
        )
        .unwrap();
        t.add_message(chat, Author::Agent, "Другой", &json!({"task": 8, "timeline": [{"kind": "step", "title": "Поиск", "tool": "search", "status": "found"}]})).unwrap();
        assert!(t.set_step_phrase(chat, 7, 1, "Читаю про отчёт").unwrap());
        assert!(!t.set_step_phrase(chat, 7, 5, "мимо").unwrap(), "no such step");
        assert!(!t.set_step_phrase(chat, 9, 0, "мимо").unwrap(), "no such answer");
        assert!(!t.set_step_phrase(chat, 7, 0, "").unwrap(), "an empty phrase is not a title");
        let messages = t.messages(chat).unwrap();
        let timeline = messages[0].timeline();
        let titles: Vec<_> = timeline.entries.iter().filter_map(|e| match e {
            TimelineEntry::Step { title, .. } => Some(title.as_str()),
            _ => None,
        }).collect();
        assert_eq!(titles, ["Поиск", "Читаю про отчёт"]);
        assert_eq!(messages[0].details["steps"][1]["title"], "Читаю про отчёт");
        assert_eq!(messages[0].details["steps"][0]["title"], "Поиск");
        assert_eq!(
            messages[1].timeline().entries[0],
            TimelineEntry::Step { title: "Поиск".into(), tool: "search".into(), status: "found".into() },
            "another answer is left as it was"
        );
    }

    #[test]
    fn sections_replace_the_streamed_draft_and_a_stop_marks_running_steps() {
        let mut tl = Timeline::default();
        tl.answer("Часть 1 черновик");
        tl.section("Часть 1.");
        tl.answer("Не то");
        tl.discard_answer();
        tl.step("Читаю", "read", STEP_RUNNING);
        tl.thinking("Думаю");
        tl.stop();
        assert_eq!(
            tl.entries,
            [
                TimelineEntry::Answer { text: "Часть 1.".into(), section: true },
                TimelineEntry::Step { title: "Читаю".into(), tool: "read".into(), status: "stopped".into() },
                TimelineEntry::Thinking { text: "Думаю".into() },
            ]
        );
        assert_eq!(tl.draft(), "");
        // A finished answer with no text adds nothing; the same text after a section is a new part.
        tl.finish_answer("  ");
        tl.finish_answer("Итог.");
        assert_eq!(tl.entries.last(), Some(&TimelineEntry::Answer { text: "Итог.".into(), section: false }));
        // `section` is written only when set.
        assert_eq!(tl.to_value()[0], json!({"kind": "answer", "text": "Часть 1.", "section": true}));
        assert_eq!(tl.to_value()[3], json!({"kind": "answer", "text": "Итог."}));
    }

    #[test]
    fn tabs_keep_chats_open_or_archived() {
        let dir = tempfile::tempdir().unwrap();
        let t = Transcript::open(dir.path(), "f").unwrap();
        let a = t.create_chat("А").unwrap();
        let b = t.create_chat("Б").unwrap();
        let c = t.create_chat("В").unwrap();
        // Without the key every chat is open, newest first, and none is archived.
        assert_eq!(t.open_chats().unwrap(), [c, b, a]);
        assert!(t.archived_chats().unwrap().is_empty());

        // Tab order is kept; chats left out are archived, newest first.
        t.set_open_chats(&[b, a]).unwrap();
        assert_eq!(t.open_chats().unwrap(), [b, a]);
        assert_eq!(t.archived_chats().unwrap().iter().map(|x| x.id).collect::<Vec<_>>(), [c]);

        // A restored chat becomes a tab again.
        t.set_open_chats(&[b, a, c]).unwrap();
        assert!(t.archived_chats().unwrap().is_empty());

        // Deleting an open chat drops its tab and its memory; the others keep their order.
        t.set_chat_memory(a, r#"{"facts":["x"]}"#).unwrap();
        t.delete_chat(a).unwrap();
        assert_eq!(t.open_chats().unwrap(), [b, c]);
        assert_eq!(t.chat_memory(a).unwrap(), None);
        assert!(t.chats().unwrap().iter().all(|x| x.id != a));

        // Deleting an archived chat leaves the open tabs as they are.
        t.set_open_chats(&[b]).unwrap();
        t.delete_chat(c).unwrap();
        assert_eq!(t.open_chats().unwrap(), [b]);
        assert!(t.archived_chats().unwrap().is_empty());
        assert_eq!(t.chats().unwrap().iter().map(|x| x.id).collect::<Vec<_>>(), [b]);
    }

    #[test]
    fn a_timeline_keeps_where_a_correction_was_taken() {
        let mut tl = Timeline::default();
        tl.thinking("Сначала сроки.");
        tl.step("Поиск", "search", "found");
        tl.steer("Ещё добавь про бюджет.");
        tl.answer("Готово.");
        assert_eq!(tl.to_value()[2], json!({"kind": "steer", "text": "Ещё добавь про бюджет."}));
        assert_eq!(Timeline::from_details(&json!({"timeline": tl.to_value()})), tl);
    }

    #[test]
    fn corrections_move_to_the_next_answer_after_the_taken_ones() {
        let dir = tempfile::tempdir().unwrap();
        let t = Transcript::open(dir.path(), "f").unwrap();
        let chat = t.create_chat("Сроки").unwrap();
        let steer = |task: i64| json!({"steer": {"task": task}});
        t.add_message(chat, Author::User, "Когда отчёт?", &serde_json::Value::Null).unwrap();
        t.add_message(chat, Author::User, "Первое", &steer(7)).unwrap();
        t.add_message(chat, Author::User, "Второе", &steer(7)).unwrap();
        t.add_message(chat, Author::User, "Третье", &steer(7)).unwrap();
        // The first was taken; the next one becomes the next question; the rest follow it.
        assert_eq!(t.retag_steers(chat, 7, 9, 2).unwrap(), 1);
        let tasks: Vec<i64> = t.messages(chat).unwrap().iter().filter_map(|m| m.details["steer"]["task"].as_i64()).collect();
        assert_eq!(tasks, [7, 7, 9]);
        // Nothing else is touched: the plain question, and the corrections left behind.
        assert_eq!(t.retag_steers(chat, 7, 11, 0).unwrap(), 2);
        let messages = t.messages(chat).unwrap();
        assert_eq!(messages[0].details, serde_json::Value::Null);
        assert_eq!(messages[2].details["steer"]["task"], 11);
        // The correction asked as the next question is a question of the chat now.
        assert!(t.unsteer(chat, 11, 0).unwrap());
        let messages = t.messages(chat).unwrap();
        assert!(messages[1].details.get("steer").is_none());
        assert_eq!(messages[2].details["steer"]["task"], 11);
    }
}
