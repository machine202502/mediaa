#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queued_cursor_with_steps_continues_only_when_there_is_no_live_stream() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let cursor = serde_json::json!({ "steps": [{ "title": "Поиск" }] });
        assert!(continued_cursor(&cursor, false));
        assert!(
            !continued_cursor(&cursor, true),
            "a yield in this process still has the stream"
        );
        assert!(!continued_cursor(&serde_json::Value::Null, false));
        assert!(!continued_cursor(
            &serde_json::json!({ "steps": [] }),
            false
        ));
    }

    #[test]
    fn a_chat_without_a_title_is_named_again_once() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let info = |id: ChatId, title: &str| ChatInfo {
            id,
            title: title.into(),
            created_ms: 1,
            messages: 1,
        };
        let user = |text: &str| ChatMessage {
            author: Author::User,
            text: text.into(),
            details: serde_json::Value::Null,
        };
        let chats = [info(5, ""), info(7, "Семь")];
        let messages = [user("  Когда отчёт? "), user("и что?")];
        let mut tried = std::collections::HashSet::new();
        assert_eq!(
            retitle_candidate(&chats, Some(5), &messages, &tried),
            Some((5, "Когда отчёт?".to_string())),
            "the first question names it"
        );
        assert_eq!(
            retitle_candidate(&chats, Some(7), &messages, &tried),
            None,
            "it has a title"
        );
        assert_eq!(
            retitle_candidate(&chats, None, &messages, &tried),
            None,
            "the new-chat tab"
        );
        assert_eq!(
            retitle_candidate(&chats, Some(5), &[], &tried),
            None,
            "nothing to name it by"
        );
        tried.insert(5);
        assert_eq!(
            retitle_candidate(&chats, Some(5), &messages, &tried),
            None,
            "only once"
        );
    }

    #[test]
    fn citations_become_seek_links() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        assert_eq!(
            linkify("В пятницу [1:05].", true),
            "В пятницу [1:05](seek:65)."
        );
        assert_eq!(
            linkify("[0:00–0:11] и [1:02:30]", true),
            "[0:00–0:11](seek:0) и [1:02:30](seek:3750)"
        );
        assert_eq!(
            linkify("уже [1:05](seek:65), [см.] и [5]", true),
            "уже [1:05](seek:65), [см.] и [5]"
        );
        assert_eq!(linkify("[незакрытая", true), "[незакрытая");
        assert_eq!(
            linkify("в [#2 1:05] и [#10 0:00–0:30]", true),
            "в [#2 1:05](seek:2:65) и [#10 0:00–0:30](seek:10:0)"
        );
        assert_eq!(
            linkify("в моменте 0:00–0:11.", true),
            "в моменте [0:00](seek:0)–[0:11](seek:11)."
        );
        assert_eq!(
            linkify("в 1:02:30, не 3.14:15, не 123:45, не 12:5", true),
            "в [1:02:30](seek:3750), не 3.14:15, не 123:45, не 12:5"
        );
        assert_eq!(
            linkify("длится 26:39, см. [#2 1:05]", false),
            "длится 26:39, см. [#2 1:05](seek:2:65)",
            "a folder: no bare times"
        );
    }

    #[test]
    fn history_pairs_questions_with_answers() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let m = |author, text: &str| ChatMessage {
            author,
            text: text.into(),
            details: serde_json::Value::Null,
        };
        let h = history(&[
            m(Author::User, "a"),
            m(Author::Agent, "b"),
            m(Author::User, "c"),
        ]);
        assert_eq!(
            h,
            vec![Turn {
                question: "a".into(),
                answer: "b".into()
            }],
            "an unanswered question is not history"
        );
        let h = history(&[
            m(Author::User, "a"),
            m(Author::Agent, "часть 1"),
            m(Author::Agent, "часть 2"),
            m(Author::User, "c"),
            m(Author::Agent, "d"),
        ]);
        assert_eq!(
            h.iter().map(|t| t.answer.as_str()).collect::<Vec<_>>(),
            ["часть 1\n\nчасть 2", "d"]
        );
    }

    #[test]
    fn tabs_close_to_a_neighbour_restore_and_start_a_new_chat() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let info = |id: ChatId, title: &str| ChatInfo {
            id,
            title: title.into(),
            created_ms: 1,
            messages: 0,
        };
        let chats = [info(5, "Пять"), info(7, "Семь"), info(9, "Девять")];
        let open = [5, 7, 9];
        assert_eq!(neighbour_after_close(&without_chat(&open, 7), 1), Some(9));
        assert_eq!(neighbour_after_close(&without_chat(&open, 9), 2), Some(7));
        assert_eq!(neighbour_after_close(&without_chat(&open, 5), 0), Some(7));
        assert_eq!(neighbour_after_close(&without_chat(&[5], 5), 0), None);
        let tabs = tab_list(&chats, &[], None, "Новый чат", &[]);
        assert_eq!(tabs.len(), 1, "only «Новый чат»");
        assert_eq!(tabs[0].id, None);
        assert!(
            tabs[0].active && !tabs[0].closable,
            "there is no stored chat to close"
        );

        let tabs = tab_list(&chats, &open, None, "Новый чат", &[]);
        assert_eq!(
            tabs.iter().map(|t| t.id).collect::<Vec<_>>(),
            [Some(5), Some(7), Some(9), None]
        );
        assert!(tabs.last().is_some_and(|t| t.active) && tabs[..3].iter().all(|t| !t.active));

        let mut open = without_chat(&open, 7);
        assert_eq!(open, [5, 9]);
        open = with_restored(&open, 7);
        assert_eq!(open, [5, 9, 7], "a closed tab does not keep its place");
        assert_eq!(with_restored(&open, 7), open, "already open");
        let tabs = tab_list(&chats, &open, Some(7), "Новый чат", &[]);
        assert_eq!(
            tabs.iter().map(|t| t.id).collect::<Vec<_>>(),
            [Some(5), Some(9), Some(7)]
        );
        assert!(tabs[2].active && tabs[..2].iter().all(|t| !t.active));
    }

    #[test]
    fn a_message_sent_while_the_answer_runs_is_a_correction_of_it() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        assert_eq!(sent_as(None), Sent::Question);
        assert_eq!(sent_as(Some(7)), Sent::Correction(7));
    }

    #[test]
    fn corrections_wait_under_the_answer_until_taken() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let steer = |task: i64, text: &str| ChatMessage {
            author: Author::User,
            text: text.into(),
            details: serde_json::json!({ "steer": { "task": task } }),
        };
        let plain = ChatMessage {
            author: Author::User,
            text: "Когда отчёт?".into(),
            details: serde_json::Value::Null,
        };
        let messages = [
            plain,
            steer(7, "Ещё про бюджет."),
            steer(7, "И про сроки."),
            steer(9, "Другое."),
        ];
        let pending = |answer, taken| queued::pending_steers(&messages, answer, taken);
        assert_eq!(
            pending(7, 0),
            [
                (1, "Ещё про бюджет.".to_string()),
                (2, "И про сроки.".to_string())
            ],
            "both under «думаю…», in order"
        );
        assert_eq!(
            pending(7, 1)
                .iter()
                .map(|(index, _)| *index)
                .collect::<Vec<_>>(),
            [2]
        );
        assert!(pending(7, 2).is_empty());
        assert_eq!(pending(9, 0).len(), 1);
    }

    #[test]
    fn a_chat_without_a_title_is_a_new_chat_until_it_is_named() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let info = |id: ChatId, title: &str| ChatInfo {
            id,
            title: title.into(),
            created_ms: 1,
            messages: 2,
        };
        let chats = [info(5, ""), info(7, "Приветствие")];
        let tabs = tab_list(&chats, &[5, 7], Some(5), "Новый чат", &[]);
        assert_eq!(
            tabs.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            ["Новый чат", "Приветствие"]
        );
        let marked = tab_list(&chats, &[5, 7], Some(5), "Новый чат", &[7]);
        assert_eq!(
            marked.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            ["Новый чат", "● Приветствие"]
        );
        assert_eq!(shown_title(""), t!("studio.new_chat"));
        assert_eq!(shown_title("Сроки"), "Сроки");
    }
}

