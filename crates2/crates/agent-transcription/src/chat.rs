use crate::transcript_db::Transcript;
pub use mediaa2_core::chat::{
    Author, ChatId, ChatInfo, TimelineEntry, OPEN_CHATS, STEP_RUNNING, STEP_STOPPED,
};
use rusqlite::{params, OptionalExtension};

pub use mediaa2_core::chat::*;

impl Transcript {
    pub fn create_chat(&self, title: &str) -> anyhow::Result<ChatId> {
        self.db.execute(
            "INSERT INTO chats (title, created_ms) VALUES (?1, ?2)",
            params![title, mediaa2_core::chat::now_ms()],
        )?;
        Ok(self.db.last_insert_rowid())
    }

    pub fn chats(&self) -> anyhow::Result<Vec<ChatInfo>> {
        let mut q = self.db.prepare(
            "SELECT c.id, c.title, c.created_ms, (SELECT COUNT(*) FROM messages m WHERE m.chat = c.id) FROM chats c ORDER BY c.id DESC",
        )?;
        let rows = q.query_map([], |r| {
            Ok(ChatInfo {
                id: r.get(0)?,
                title: r.get(1)?,
                created_ms: r.get(2)?,
                messages: r.get::<_, i64>(3)? as usize,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn open_chats(&self) -> anyhow::Result<Vec<ChatId>> {
        if let Some(json) = self.meta(OPEN_CHATS)? {
            match serde_json::from_str::<Vec<ChatId>>(&json) {
                Ok(ids) => return Ok(ids),
                Err(e) => log::warn!("chat-tabs: {e}; treating every chat as open"),
            }
        }
        Ok(self.chats()?.into_iter().map(|c| c.id).collect())
    }

    pub fn set_open_chats(&self, chats: &[ChatId]) -> anyhow::Result<()> {
        self.set_meta(OPEN_CHATS, &serde_json::to_string(chats)?)
    }

    /// The model pinned to this chat, stored as a JSON `ModelSource`.
    pub fn chat_model(
        &self,
        chat: ChatId,
    ) -> anyhow::Result<Option<mediaa2_core::agent::ModelSource>> {
        let json: Option<String> = self
            .db
            .query_row("SELECT model FROM chats WHERE id = ?1", [chat], |r| {
                r.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten();
        Ok(json
            .as_deref()
            .filter(|j| !j.trim().is_empty())
            .and_then(|j| serde_json::from_str(j).ok()))
    }

    pub fn set_chat_model(
        &self,
        chat: ChatId,
        model: &mediaa2_core::agent::ModelSource,
    ) -> anyhow::Result<()> {
        let json = serde_json::to_string(model)?;
        self.db.execute(
            "UPDATE chats SET model = ?1 WHERE id = ?2",
            params![json, chat],
        )?;
        Ok(())
    }

    pub fn archived_chats(&self) -> anyhow::Result<Vec<ChatInfo>> {
        let open = self.open_chats()?;
        Ok(self
            .chats()?
            .into_iter()
            .filter(|c| !open.contains(&c.id))
            .collect())
    }

    pub fn chat_title(&self, chat: ChatId) -> anyhow::Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT title FROM chats WHERE id = ?1", [chat], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_chat_title(&self, chat: ChatId, title: &str) -> anyhow::Result<bool> {
        let title = title.trim();
        if title.is_empty() {
            return Ok(false);
        }
        let changed = self.db.execute(
            "UPDATE chats SET title = ?1 WHERE id = ?2 AND title = ''",
            params![title, chat],
        )?;
        Ok(changed > 0)
    }

    pub fn delete_chat(&self, chat: ChatId) -> anyhow::Result<()> {
        self.db.execute("DELETE FROM chats WHERE id = ?1", [chat])?;
        self.db
            .execute("DELETE FROM meta WHERE key = ?1", [memory_key(chat)])?;
        let open = self.open_chats()?;
        if open.contains(&chat) {
            let left: Vec<ChatId> = open.into_iter().filter(|&c| c != chat).collect();
            self.set_open_chats(&left)?;
        }
        Ok(())
    }

    pub fn chat_memory(&self, chat: ChatId) -> anyhow::Result<Option<String>> {
        self.meta(&memory_key(chat))
    }

    pub fn set_chat_memory(&self, chat: ChatId, memory: &str) -> anyhow::Result<()> {
        self.set_meta(&memory_key(chat), memory)
    }

    pub fn add_message(
        &self,
        chat: ChatId,
        author: Author,
        text: &str,
        details: &serde_json::Value,
    ) -> anyhow::Result<()> {
        let role = match author {
            Author::User => "user",
            Author::Agent => "agent",
        };
        self.db.execute(
            "INSERT INTO messages (chat, role, text, details, created_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![chat, role, text, details.to_string(), mediaa2_core::chat::now_ms()],
        )?;
        Ok(())
    }

    pub fn messages(&self, chat: ChatId) -> anyhow::Result<Vec<ChatMessage>> {
        let mut q = self
            .db
            .prepare("SELECT role, text, details FROM messages WHERE chat = ?1 ORDER BY id")?;
        let rows = q.query_map([chat], |r| {
            let role: String = r.get(0)?;
            let details: String = r.get(2)?;
            Ok(ChatMessage {
                author: if role == "user" {
                    Author::User
                } else {
                    Author::Agent
                },
                text: r.get(1)?,
                details: serde_json::from_str(&details).unwrap_or(serde_json::Value::Null),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_step_phrase(
        &self,
        chat: ChatId,
        answer: i64,
        index: usize,
        phrase: &str,
    ) -> anyhow::Result<bool> {
        if phrase.is_empty() {
            return Ok(false);
        }
        let rows = {
            let mut q = self.db.prepare("SELECT id, details FROM messages WHERE chat = ?1 AND role = 'agent' ORDER BY id DESC")?;
            let mapped = q.query_map([chat], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        for (id, raw) in rows {
            let mut details: serde_json::Value =
                serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
            if details.get("task").and_then(|v| v.as_i64()) != Some(answer) {
                continue;
            }
            let mut timeline = Timeline::from_details(&details);
            if !timeline.set_step_title(index, phrase) {
                return Ok(false);
            }
            details["timeline"] = timeline.to_value();
            if let Some(step) = details
                .get_mut("steps")
                .and_then(|v| v.as_array_mut())
                .and_then(|steps| steps.get_mut(index))
                .and_then(|v| v.as_object_mut())
            {
                step.insert(
                    "title".to_string(),
                    serde_json::Value::String(phrase.to_string()),
                );
            }
            self.db.execute(
                "UPDATE messages SET details = ?1 WHERE id = ?2",
                params![details.to_string(), id],
            )?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn unsteer(&self, chat: ChatId, from: i64, mut skip: usize) -> anyhow::Result<bool> {
        let rows = {
            let mut q = self.db.prepare(
                "SELECT id, details FROM messages WHERE chat = ?1 AND role = 'user' ORDER BY id",
            )?;
            let mapped = q.query_map([chat], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        for (id, raw) in rows {
            let mut details: serde_json::Value =
                serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
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
            self.db.execute(
                "UPDATE messages SET details = ?1 WHERE id = ?2",
                params![details.to_string(), id],
            )?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn retag_steers(
        &self,
        chat: ChatId,
        from: i64,
        to: i64,
        mut skip: usize,
    ) -> anyhow::Result<usize> {
        let rows = {
            let mut q = self.db.prepare(
                "SELECT id, details FROM messages WHERE chat = ?1 AND role = 'user' ORDER BY id",
            )?;
            let mapped = q.query_map([chat], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        let mut moved = 0;
        for (id, raw) in rows {
            let mut details: serde_json::Value =
                serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
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
            self.db.execute(
                "UPDATE messages SET details = ?1 WHERE id = ?2",
                params![details.to_string(), id],
            )?;
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
    fn a_chat_keeps_the_model_it_started_with() {
        let dir = tempfile::tempdir().unwrap();
        let t = Transcript::open(dir.path(), "f").unwrap();
        let chat = t.create_chat("Chat").unwrap();
        assert_eq!(t.chat_model(chat).unwrap(), None);

        let source = mediaa2_core::agent::ModelSource::new("c1", "gpt-x");
        t.set_chat_model(chat, &source).unwrap();
        let stored = t.chat_model(chat).unwrap().expect("the pinned model");
        assert_eq!(
            (stored.connection.as_str(), stored.model.as_str()),
            ("c1", "gpt-x")
        );

        let switched = mediaa2_core::agent::ModelSource::new("c2", "deepseek-chat");
        t.set_chat_model(chat, &switched).unwrap();
        assert_eq!(t.chat_model(chat).unwrap().unwrap().model, "deepseek-chat");
    }

    #[test]
    fn a_v5_database_keeps_its_chats_and_gains_the_model_column() {
        let dir = tempfile::tempdir().unwrap();
        let path = crate::transcript_db::db_path(dir.path(), "f");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            let db = rusqlite::Connection::open(&path).unwrap();
            db.pragma_update(None, "user_version", 5).unwrap();
            db.execute_batch(
                "CREATE TABLE chats (id INTEGER PRIMARY KEY, title TEXT NOT NULL, created_ms INTEGER NOT NULL);
                 CREATE TABLE messages (id INTEGER PRIMARY KEY, chat INTEGER NOT NULL, role TEXT NOT NULL, text TEXT NOT NULL, details TEXT NOT NULL, created_ms INTEGER NOT NULL);
                 INSERT INTO chats (title, created_ms) VALUES ('old', 1);",
            )
            .unwrap();
        }
        let t = Transcript::open(dir.path(), "f").unwrap();
        let chats = t.chats().unwrap();
        assert_eq!(chats.len(), 1, "the v5 chat survives the migration");
        assert_eq!(t.chat_model(chats[0].id).unwrap(), None);
        let source = mediaa2_core::agent::ModelSource::new("c", "m");
        t.set_chat_model(chats[0].id, &source).unwrap();
        assert_eq!(t.chat_model(chats[0].id).unwrap().unwrap().model, "m");
    }

    #[test]
    fn chats_keep_their_messages() {
        let dir = tempfile::tempdir().unwrap();
        let t = Transcript::open(dir.path(), "f").unwrap();
        let a = t.create_chat("Сроки").unwrap();
        let b = t.create_chat("Бюджет").unwrap();
        t.add_message(a, Author::User, "Когда отчёт?", &serde_json::Value::Null)
            .unwrap();
        t.add_message(
            a,
            Author::Agent,
            "В пятницу [0:10].",
            &json!([{"tool": "search"}]),
        )
        .unwrap();
        let chats = t.chats().unwrap();
        assert_eq!(
            chats
                .iter()
                .map(|c| (c.title.as_str(), c.messages))
                .collect::<Vec<_>>(),
            [("Бюджет", 0), ("Сроки", 2)],
            "newest first"
        );
        assert!(chats.iter().all(|c| c.created_ms > 0));
        let m = t.messages(a).unwrap();
        assert_eq!((m[0].author, m[1].author), (Author::User, Author::Agent));
        assert_eq!(m[1].details[0]["tool"], "search");
        t.set_chat_memory(a, r#"{"facts":["x"]}"#).unwrap();
        assert_eq!(
            t.chat_memory(a).unwrap().as_deref(),
            Some(r#"{"facts":["x"]}"#)
        );
        assert_eq!(t.chat_memory(b).unwrap(), None);
        let fresh = t.create_chat("").unwrap();
        assert!(
            !t.set_chat_title(fresh, "  ").unwrap(),
            "an empty title is nothing"
        );
        assert!(t.set_chat_title(fresh, " Деньги ").unwrap());
        assert!(!t.set_chat_title(fresh, "Другое").unwrap());
        assert!(!t.set_chat_title(b, "Другое").unwrap());
        assert_eq!(t.chat_title(fresh).unwrap().as_deref(), Some("Деньги"));
        assert_eq!(t.chat_title(b).unwrap().as_deref(), Some("Бюджет"));
        t.delete_chat(fresh).unwrap();
        t.delete_chat(a).unwrap();
        assert!(
            t.messages(a).unwrap().is_empty(),
            "messages go with the chat"
        );
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
        tl.step("Проверяю", "read", STEP_RUNNING);
        assert_eq!(tl.draft(), "");
        tl.answer("И ещё.");
        tl.finish_answer("И ещё [0:10].");
        assert_eq!(
            tl.entries,
            [
                TimelineEntry::Thinking {
                    text: "Надо поискать.".into()
                },
                TimelineEntry::Step {
                    title: "Ищу сроки".into(),
                    tool: "search".into(),
                    status: "found".into()
                },
                TimelineEntry::Thinking {
                    text: "Нашёл.".into()
                },
                TimelineEntry::Answer {
                    text: "В пятницу.".into(),
                    section: false
                },
                TimelineEntry::Step {
                    title: "Проверяю".into(),
                    tool: "read".into(),
                    status: "running".into()
                },
                TimelineEntry::Answer {
                    text: "И ещё [0:10].".into(),
                    section: false
                },
            ]
        );

        let dir = tempfile::tempdir().unwrap();
        let t = Transcript::open(dir.path(), "f").unwrap();
        let chat = t.create_chat("Сроки").unwrap();
        t.add_message(
            chat,
            Author::Agent,
            "И ещё [0:10].",
            &json!({"steps": [], "timeline": tl.to_value(), "stopped": true}),
        )
        .unwrap();
        t.add_message(chat, Author::Agent, "Старый ответ", &json!({"steps": []}))
            .unwrap();
        let messages = t.messages(chat).unwrap();
        assert_eq!(
            messages[0].timeline(),
            tl,
            "the saved details give the same timeline"
        );
        assert!(messages[0].stopped() && !messages[1].stopped());
        assert!(
            messages[1].timeline().is_empty(),
            "an answer from before timelines has none"
        );
    }

    #[test]
    fn sibling_steps_of_one_call_batch_end_in_their_own_order() {
        let mut tl = Timeline::default();
        tl.step("Поиск", "search", STEP_RUNNING);
        tl.step("Чтение записи", "read", STEP_RUNNING);
        assert_eq!(tl.draft(), "");
        tl.finish_step("found");
        tl.finish_step("nothing");
        assert_eq!(
            tl.entries,
            [
                TimelineEntry::Step {
                    title: "Поиск".into(),
                    tool: "search".into(),
                    status: "found".into()
                },
                TimelineEntry::Step {
                    title: "Чтение записи".into(),
                    tool: "read".into(),
                    status: "nothing".into()
                },
            ]
        );
        tl.step("Спикеры", "speakers", STEP_RUNNING);
        tl.finish_step("found");
        tl.finish_step("failed");
        assert_eq!(
            tl.entries.last(),
            Some(&TimelineEntry::Step {
                title: "Спикеры".into(),
                tool: "speakers".into(),
                status: "failed".into()
            })
        );
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
        assert!(
            !t.set_step_phrase(chat, 7, 5, "мимо").unwrap(),
            "no such step"
        );
        assert!(
            !t.set_step_phrase(chat, 9, 0, "мимо").unwrap(),
            "no such answer"
        );
        assert!(
            !t.set_step_phrase(chat, 7, 0, "").unwrap(),
            "an empty phrase is not a title"
        );
        let messages = t.messages(chat).unwrap();
        let timeline = messages[0].timeline();
        let titles: Vec<_> = timeline
            .entries
            .iter()
            .filter_map(|e| match e {
                TimelineEntry::Step { title, .. } => Some(title.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(titles, ["Поиск", "Читаю про отчёт"]);
        assert_eq!(messages[0].details["steps"][1]["title"], "Читаю про отчёт");
        assert_eq!(messages[0].details["steps"][0]["title"], "Поиск");
        assert_eq!(
            messages[1].timeline().entries[0],
            TimelineEntry::Step {
                title: "Поиск".into(),
                tool: "search".into(),
                status: "found".into()
            },
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
                TimelineEntry::Answer {
                    text: "Часть 1.".into(),
                    section: true
                },
                TimelineEntry::Step {
                    title: "Читаю".into(),
                    tool: "read".into(),
                    status: "stopped".into()
                },
                TimelineEntry::Thinking {
                    text: "Думаю".into()
                },
            ]
        );
        assert_eq!(tl.draft(), "");
        tl.finish_answer("  ");
        tl.finish_answer("Итог.");
        assert_eq!(
            tl.entries.last(),
            Some(&TimelineEntry::Answer {
                text: "Итог.".into(),
                section: false
            })
        );
        assert_eq!(
            tl.to_value()[0],
            json!({"kind": "answer", "text": "Часть 1.", "section": true})
        );
        assert_eq!(tl.to_value()[3], json!({"kind": "answer", "text": "Итог."}));
    }

    #[test]
    fn tabs_keep_chats_open_or_archived() {
        let dir = tempfile::tempdir().unwrap();
        let t = Transcript::open(dir.path(), "f").unwrap();
        let a = t.create_chat("А").unwrap();
        let b = t.create_chat("Б").unwrap();
        let c = t.create_chat("В").unwrap();
        assert_eq!(t.open_chats().unwrap(), [c, b, a]);
        assert!(t.archived_chats().unwrap().is_empty());

        t.set_open_chats(&[b, a]).unwrap();
        assert_eq!(t.open_chats().unwrap(), [b, a]);
        assert_eq!(
            t.archived_chats()
                .unwrap()
                .iter()
                .map(|x| x.id)
                .collect::<Vec<_>>(),
            [c]
        );

        t.set_open_chats(&[b, a, c]).unwrap();
        assert!(t.archived_chats().unwrap().is_empty());

        t.set_chat_memory(a, r#"{"facts":["x"]}"#).unwrap();
        t.delete_chat(a).unwrap();
        assert_eq!(t.open_chats().unwrap(), [b, c]);
        assert_eq!(t.chat_memory(a).unwrap(), None);
        assert!(t.chats().unwrap().iter().all(|x| x.id != a));

        t.set_open_chats(&[b]).unwrap();
        t.delete_chat(c).unwrap();
        assert_eq!(t.open_chats().unwrap(), [b]);
        assert!(t.archived_chats().unwrap().is_empty());
        assert_eq!(
            t.chats().unwrap().iter().map(|x| x.id).collect::<Vec<_>>(),
            [b]
        );
    }

    #[test]
    fn a_timeline_keeps_where_a_correction_was_taken() {
        let mut tl = Timeline::default();
        tl.thinking("Сначала сроки.");
        tl.step("Поиск", "search", "found");
        tl.steer("Ещё добавь про бюджет.");
        tl.answer("Готово.");
        assert_eq!(
            tl.to_value()[2],
            json!({"kind": "steer", "text": "Ещё добавь про бюджет."})
        );
        assert_eq!(
            Timeline::from_details(&json!({"timeline": tl.to_value()})),
            tl
        );
    }

    #[test]
    fn corrections_move_to_the_next_answer_after_the_taken_ones() {
        let dir = tempfile::tempdir().unwrap();
        let t = Transcript::open(dir.path(), "f").unwrap();
        let chat = t.create_chat("Сроки").unwrap();
        let steer = |task: i64| json!({"steer": {"task": task}});
        t.add_message(chat, Author::User, "Когда отчёт?", &serde_json::Value::Null)
            .unwrap();
        t.add_message(chat, Author::User, "Первое", &steer(7))
            .unwrap();
        t.add_message(chat, Author::User, "Второе", &steer(7))
            .unwrap();
        t.add_message(chat, Author::User, "Третье", &steer(7))
            .unwrap();
        assert_eq!(t.retag_steers(chat, 7, 9, 2).unwrap(), 1);
        let tasks: Vec<i64> = t
            .messages(chat)
            .unwrap()
            .iter()
            .filter_map(|m| m.details["steer"]["task"].as_i64())
            .collect();
        assert_eq!(tasks, [7, 7, 9]);
        assert_eq!(t.retag_steers(chat, 7, 11, 0).unwrap(), 2);
        let messages = t.messages(chat).unwrap();
        assert_eq!(messages[0].details, serde_json::Value::Null);
        assert_eq!(messages[2].details["steer"]["task"], 11);
        assert!(t.unsteer(chat, 11, 0).unwrap());
        let messages = t.messages(chat).unwrap();
        assert!(messages[1].details.get("steer").is_none());
        assert_eq!(messages[2].details["steer"]["task"], 11);
    }
}
