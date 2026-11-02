#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::transcript::{NameEvidence, Word};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn an_automatic_name_says_what_shows_it() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let auto = SpeakerName {
            name: "Иван".into(),
            source: NameSource::Auto,
            evidence: vec![NameEvidence {
                start: 5.0,
                text: "Меня зовут Иван.".into(),
                how: "self".into(),
            }],
        };
        assert_eq!(auto_evidence(&auto).unwrap(), "[0:05] Меня зовут Иван.");
        assert!(auto_evidence(&SpeakerName {
            source: NameSource::Manual,
            ..auto.clone()
        })
        .is_none());
        assert!(auto_evidence(&SpeakerName {
            evidence: Vec::new(),
            ..auto
        })
        .is_none());
    }

    fn word(text: &str, start: f64) -> Word {
        Word {
            text: text.into(),
            start,
            end: start + 0.3,
        }
    }

    #[test]
    fn clicks_find_words() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let r = Reply {
            start: 0.0,
            end: 2.0,
            speaker: None,
            words: vec![word("привет,", 0.0), word("мир", 1.0)],
        };
        assert_eq!(word_at(&r, 0), Some(0));
        assert_eq!(word_at(&r, 6), Some(0));
        assert_eq!(word_at(&r, 7), Some(0), "the space after a word");
        assert_eq!(word_at(&r, 8), Some(1));
        assert_eq!(word_at(&r, 11), Some(1), "after the last letter");
        assert_eq!(word_at(&r, 12), None);
    }

    #[test]
    fn sync_follows_only_what_left_the_view_and_names_save_together() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        assert!(click_seeks(true) && !click_seeks(false));
        assert!(
            !should_follow(false, true, false, true),
            "off: no scroll and no jump"
        );
        assert!(
            should_follow(true, true, false, false),
            "a reply that left the view"
        );
        assert!(
            !should_follow(true, true, true, false),
            "a reply still in view stays put"
        );
        assert!(
            should_follow(true, false, true, true),
            "turning sync on jumps to the current place"
        );
        assert!(!should_follow(true, false, false, false));
        assert!(word_on(1.0, 1.0, 1.2));
        assert!(!word_on(0.5, 1.0, 1.2));
        assert!(
            word_on(1.1, 1.0, 1.05),
            "a short word stays lit a little past its end"
        );
        let replies = [
            Reply {
                start: 1.0,
                end: 3.0,
                speaker: Some(1),
                words: vec![],
            },
            Reply {
                start: 0.5,
                end: 1.5,
                speaker: Some(0),
                words: vec![],
            },
            Reply {
                start: 4.0,
                end: 5.0,
                speaker: None,
                words: vec![],
            },
            Reply {
                start: 6.0,
                end: 6.5,
                speaker: Some(0),
                words: vec![],
            },
        ];
        let facts = speaker_facts(&replies);
        assert_eq!(facts.len(), 2);
        assert_eq!(
            facts[0],
            SpeakerFact {
                id: 0,
                spoken: 1.5,
                first: 0.5
            }
        );
        assert_eq!(
            facts[1],
            SpeakerFact {
                id: 1,
                spoken: 2.0,
                first: 1.0
            }
        );
        let mut names = BTreeMap::from([(0, "Оля".to_string())]);
        names = apply_names(&names, &[(0, "  ".into()), (1, " Боря ".into())]);
        assert!(!names.contains_key(&0), "an empty field removes the name");
        assert_eq!(names.get(&1).map(String::as_str), Some("Боря"));
    }

    #[test]
    fn the_latest_task_of_the_file() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        use mediaa_queue::{FakeClock, MemStore, NewTask, Resource, Store};
        use std::sync::Arc;
        let q = MemStore::new(Arc::new(FakeClock::new(0)));
        let new = |title: &str, scope: &str| {
            NewTask::new(pipeline::ANALYSIS_KIND, title, &json!({}))
                .composite()
                .scope(scope)
        };
        q.enqueue(new("a", "a")).unwrap();
        q.enqueue(new("b", "b")).unwrap();
        let again = q.enqueue(new("a", "a")).unwrap();
        q.enqueue(
            NewTask::new("other", "a", &json!({}))
                .resource(Resource::Llm)
                .scope("a"),
        )
        .unwrap();
        let tasks = q.list().unwrap();
        assert_eq!(task_of(&tasks, "a").map(|t| t.id), Some(again));
        assert!(task_of(&tasks, "c").is_none());
        assert_ne!(speaker_color(0), speaker_color(1));
        assert_eq!(speaker_color(0), speaker_color(SPEAKER_COLORS.len() as u32));
    }

    #[test]
    fn an_event_reloads_the_recording() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        use crate::logic::agent_host::AgentServices;
        use mediaa_agent::run::{Feed, Streams};
        use mediaa_core::agent::AgentSettings;
        use mediaa_core::transcript::Word;
        use mediaa_engine::testing::FakeEngine;
        use mediaa_queue::{Event, EventKind, FakeClock, MemStore, Scheduler};
        use std::sync::{Arc, RwLock};

        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path().join(".ai");
        std::fs::create_dir_all(&ai).unwrap();
        let media = dir.path().join("talk.bin");
        std::fs::write(&media, b"studio-event").unwrap();
        let mut dialog = Dialog::new(&media, &ai);
        dialog.refresh(None);
        assert!(dialog.replies.is_empty());
        let id = dialog.file_id.clone().unwrap();
        Transcript::open(&ai, &id)
            .unwrap()
            .set_replies(&[Reply {
                start: 0.0,
                end: 1.0,
                speaker: None,
                words: vec![Word {
                    text: "привет".into(),
                    start: 0.0,
                    end: 0.4,
                }],
            }])
            .unwrap();
        dialog.refresh(None);
        assert!(dialog.replies.is_empty(), "nothing new was announced");

        let feed = Arc::new(Feed::default());
        let store = Arc::new(MemStore::new(Arc::new(FakeClock::new(0))));
        let services = AgentServices {
            ai_dir: ai.clone(),
            engine: Arc::new(FakeEngine::new()),
            settings: Arc::new(RwLock::new(AgentSettings::default())),
            sched: Arc::new(Scheduler::new(store, mediaa_agent::run::pools())),
            streams: Arc::new(Streams::default()),
            feed: feed.clone(),
        };
        feed.push(Event {
            task: 1,
            scope: Some("other".into()),
            kind: EventKind::Done,
        });
        dialog.refresh(Some(&services));
        assert!(
            dialog.replies.is_empty(),
            "another recording's event is not this one"
        );
        feed.push(Event {
            task: 2,
            scope: Some(id),
            kind: EventKind::Done,
        });
        dialog.refresh(Some(&services));
        assert_eq!(dialog.replies.len(), 1);
        assert_eq!(dialog.replies[0].text(), "привет");
    }
}

