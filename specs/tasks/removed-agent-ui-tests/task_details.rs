#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_queue::Role;
    use mediaa_queue::{Stats, Status as TaskStatus};
    use serde_json::json;

    fn task(error: &str) -> Task {
        Task {
            id: 7,
            kind: "index".into(),
            title: "interview.mp4".into(),
            input: json!({"path": "C:/rec/interview.mp4", "options": {"topics": true}}),
            priority: 0,
            status: TaskStatus::Failed,
            cursor: Value::Null,
            partial: Value::Null,
            result: None,
            progress: 0.4,
            error: Some(error.into()),
            note: None,
            attempts: 1,
            created_ms: 0,
            updated_ms: 4_000,
            parent: Some(1),
            scope: Some("file-1".into()),
            inline: false,
            started_ms: Some(1_000),
            finished_ms: Some(4_000),
            stats: Stats {
                model_calls: 3,
                prompt_tokens: 100,
                completion_tokens: 20,
                model_ms: 500,
                models: vec!["Qwen/Qwen3.5-4B".into()],
            },
            detail: Value::Null,
            units: 1.0,
            resource: None,
            memory: 0,
            model: None,
            role: Role::Work,
            gen: 0,
            yield_cont: false,
            heartbeat_ms: None,
            stop_intent: None,
        }
    }

    #[test]
    fn the_facts_name_the_task_its_input_and_its_models() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let facts = facts_of(&task("boom"), &task("boom").stats, 5_000, "ru");
        assert_eq!(facts.label, "Индекс: interview.mp4");
        assert_eq!(facts.status, "ошибка");
        assert_eq!(facts.kind, "index");
        assert_eq!(facts.recording, "interview.mp4");
        assert_eq!(facts.scope, "file-1");
        assert_eq!(facts.path, "C:/rec/interview.mp4");
        assert_eq!(facts.options, r#"{"topics":true}"#);
        assert_eq!(facts.models, "Qwen/Qwen3.5-4B");
        assert_eq!(facts.duration, "0:03");
        assert_eq!(facts.progress, "40 %");
        assert!(facts.stats.contains('3'), "{}", facts.stats);
    }

    #[test]
    fn a_running_task_counts_its_duration_up_to_now() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let mut t = task("");
        t.status = TaskStatus::Running;
        t.finished_ms = None;
        assert_eq!(facts_of(&t, &t.stats, 90_000, "ru").duration, "1:29");
        t.finished_ms = Some(2_000);
        let running = facts_of(&t, &t.stats, 90_000, "ru");
        assert_eq!(
            (running.duration.as_str(), running.finished.as_str()),
            ("1:29", "")
        );
        t.status = TaskStatus::Done;
        t.finished_ms = Some(4_000);
        assert_eq!(facts_of(&t, &t.stats, 90_000, "ru").duration, "0:03");
    }

    #[test]
    fn the_reason_the_watchdog_interrupted_the_task_is_shown() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let mut t = task("");
        t.status = TaskStatus::Queued;
        t.detail = json!({"hang": {"kind": "hang", "resource": "llm", "idle_s": 300}});
        let facts = facts_of(&t, &t.stats, 5_000, "ru");
        assert_eq!(
            facts.hang,
            "модель не отвечает дольше 5 мин — вызов прерван, задача продолжит с последней точки"
        );
        let report = report_text(&facts, "", "", "ru");
        assert!(
            report.contains("Прервано") && report.contains("не отвечает"),
            "{report}"
        );
        assert!(
            facts_of(&t, &t.stats, 5_000, "en")
                .hang
                .starts_with("the model has not responded"),
            "{}",
            facts_of(&t, &t.stats, 5_000, "en").hang
        );
        t.detail = Value::Null;
        assert_eq!(facts_of(&t, &t.stats, 5_000, "ru").hang, "");
    }

    #[test]
    fn a_note_is_shown_instead_of_an_older_error_and_in_the_window_language() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let mut t = task("boom");
        t.note = Some(mediaa_queue::failure_note("Темы: rec"));
        assert_eq!(
            facts_of(&t, &t.stats, 5_000, "ru").error,
            "из-за ошибки в Темы: rec"
        );
        assert_eq!(
            facts_of(&t, &t.stats, 5_000, "en").error,
            "because of a failure in Темы: rec"
        );
        t.note = None;
        assert_eq!(facts_of(&t, &t.stats, 5_000, "ru").error, "boom");
    }

    #[test]
    fn a_cut_topic_detail_says_why_it_stopped() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let mut t = task("");
        t.kind = "topic-detail".into();
        t.result = Some(json!({
            "cut": "cut at line 103 after 2 complete sections: hit the limit of 1819 tokens"
        }));
        let ru = facts_of(&t, &t.stats, 5_000, "ru");
        assert!(
            ru.cut.contains("строке 103") && ru.cut.contains("1819"),
            "{}",
            ru.cut
        );
        assert!(!ru.cut.contains("complete sections"), "{}", ru.cut);
        let en = facts_of(&t, &t.stats, 5_000, "en").cut;
        assert!(
            en.contains("line 103") && en.contains("limit") && en.contains("1819"),
            "{en}"
        );
        let report = report_text(&facts_of(&t, &t.stats, 5_000, "en"), "", "", "en");
        assert!(report.contains(&en), "{report}");

        t.result = Some(json!({
            "cut": "the first answer was cut at line 40: hit the context window of 8192 tokens"
        }));
        let text = facts_of(&t, &t.stats, 5_000, "en").cut;
        assert!(
            text.contains("first answer") && text.contains("40") && text.contains("8192"),
            "{text}"
        );
        t.result = Some(json!({
            "cut": "the prompt (~7000 tokens) and the answer (2000 tokens) do not fit the context of 8192 tokens"
        }));
        let text = facts_of(&t, &t.stats, 5_000, "ru").cut;
        assert!(
            text.contains("7000") && text.contains("2000") && text.contains("8192"),
            "{text}"
        );
        t.result = Some(json!({ "cut": "something new" }));
        assert_eq!(facts_of(&t, &t.stats, 5_000, "ru").cut, "something new");
        t.result = None;
        assert_eq!(facts_of(&t, &t.stats, 5_000, "ru").cut, "");
    }

    #[test]
    fn a_speaker_names_run_lists_the_names_and_the_evidence() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let mut t = task("");
        t.kind = "speaker-names".into();
        t.status = TaskStatus::Done;
        t.result = Some(json!({
            "names": [{ "speaker": "@s1", "name": "Иван", "score": 3.0 }],
            "evidence": [{ "speaker": "@s1", "name": "Иван", "how": "self", "time": "0:05", "text": "Меня зовут Иван.", "sure": "high" }],
            "read_secs": 720.0,
            "total_secs": 1620.0,
            "stopped_early": true,
        }));
        let facts = facts_of(&t, &t.stats, 5_000, "ru");
        assert_eq!(facts.names, "@s1 — Иван");
        assert!(
            facts.evidence.contains("[0:05]") && facts.evidence.contains("Меня зовут Иван."),
            "{}",
            facts.evidence
        );
        assert!(
            facts.read.contains("12") && facts.read.contains("27"),
            "{}",
            facts.read
        );
        let report = report_text(&facts, "", "", "ru");
        assert!(
            report.contains(&facts.evidence) && report.contains(&facts.read),
            "{report}"
        );
        t.result.as_mut().unwrap()["stopped_early"] = json!(false);
        let en = facts_of(&t, &t.stats, 5_000, "en");
        assert!(
            en.read.contains("12") && en.read.contains("27"),
            "{}",
            en.read
        );
        assert!(
            !en.read
                .contains(&t!("details.read_early", locale = "en").to_string()),
            "{}",
            en.read
        );
        let plain = task("");
        let facts = facts_of(&plain, &plain.stats, 5_000, "ru");
        assert!(facts.names.is_empty() && facts.evidence.is_empty() && facts.read.is_empty());
    }

    #[test]
    fn speakers_count_as_the_input_when_there_are_no_options() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let mut t = task("x");
        t.input = json!({"path": "a.wav", "speakers": 2});
        assert_eq!(
            facts_of(&t, &t.stats, 5_000, "en").options,
            r#"{"speakers":2}"#
        );
    }

    #[test]
    fn known_failures_get_a_hint() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        assert_eq!(
            hint_for(r"C:\models\silero_vad.onnx: the voice activity model does not load"),
            Some(Hint::ModelMissing)
        );
        assert_eq!(
            hint_for("models/x.gguf not found"),
            Some(Hint::ModelMissing)
        );
        assert_eq!(
            hint_for("the model ran out of memory"),
            Some(Hint::OutOfMemory)
        );
        assert_eq!(
            hint_for("proxy \"http://127.0.0.1:9\": connection timed out"),
            Some(Hint::Network)
        );
        assert_eq!(hint_for("the grammar was rejected"), None);
        assert_eq!(
            hint_for("proxy set, but model.onnx not found"),
            Some(Hint::ModelMissing)
        );
    }

    #[test]
    fn the_model_hint_tells_the_user_to_download() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let ru = hint_text(Hint::ModelMissing, "ru");
        assert!(ru.contains("Скачать"), "{ru}");
        assert!(!ru.contains("details."), "{ru}");
        let en = hint_text(Hint::ModelMissing, "en");
        assert!(en.to_lowercase().contains("download"), "{en}");
    }

    #[test]
    fn the_report_carries_the_error_the_hint_and_the_log() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let sample = task("model.onnx: the voice activity model does not load");
        let facts = facts_of(&sample, &sample.stats, 5_000, "en");
        let hint = hint_text(Hint::ModelMissing, "en");
        let log = "2026-10-21 21:07:01.123 [ERROR] mediaa_agent::worker: task 7 index (interview.mp4) failed: model.onnx: the voice activity model does not load\n";
        let text = report_text(&facts, &hint, log, "en");
        assert!(text.contains("interview.mp4"), "{text}");
        assert!(text.contains("does not load"), "{text}");
        assert!(text.contains(&hint), "{text}");
        assert!(text.contains("task 7 index"), "{text}");
        assert!(text.contains("Qwen/Qwen3.5-4B"), "{text}");
    }

    #[test]
    fn the_log_excerpt_keeps_the_failure_and_its_neighbours() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let log = "\
2026-10-21 21:00:00.000 [INFO] mediaa: started\n\
2026-10-21 21:06:50.000 [INFO] mediaa_engine: loading\n\
2026-10-21 21:07:01.100 [ERROR] mediaa_agent::worker: task 7 index (interview.mp4) failed: model.onnx: the voice activity model does not load\n\
caused by: file missing\n\
2026-10-21 21:07:02.000 [INFO] mediaa: still up\n\
2026-10-21 22:00:00.000 [INFO] mediaa: later\n";
        let around =
            NaiveDateTime::parse_from_str("2026-10-21 21:07:01.100", "%Y-%m-%d %H:%M:%S%.3f")
                .unwrap();
        let excerpt = log_excerpt(log, 7, Some(around));
        assert!(excerpt.contains("task 7 index"), "{excerpt}");
        assert!(excerpt.contains("caused by: file missing"), "{excerpt}");
        assert!(excerpt.contains("still up"), "{excerpt}");
        assert!(!excerpt.contains("later"), "{excerpt}");
        assert!(!excerpt.contains("started"), "{excerpt}");
    }

    #[test]
    fn an_empty_window_shows_the_records_nearest_the_failure() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let mut log = String::new();
        for i in 0..30 {
            log.push_str(&format!(
                "2026-10-21 18:39:{i:02}.000 [INFO] mediaa: startup {i}\n"
            ));
        }
        log.push_str("2026-10-21 18:58:00.000 [INFO] mediaa: last before the gap\n");
        log.push_str(&format!(
            "2026-10-21 21:40:58.000 [INFO] mediaa: nearest line {}\n",
            "A".repeat(2000)
        ));
        let around =
            NaiveDateTime::parse_from_str("2026-10-21 20:55:56.000", "%Y-%m-%d %H:%M:%S%.3f")
                .unwrap();
        let excerpt = log_excerpt(&log, 7, Some(around));
        assert!(excerpt.contains("nearest line"), "{excerpt}");
        assert!(excerpt.contains("last before the gap"), "{excerpt}");
        assert!(!excerpt.contains("startup 0"), "{excerpt}");
        assert!(excerpt.contains('…'), "{excerpt}");
        assert!(excerpt.len() < 2000, "{}", excerpt.len());
    }

    #[test]
    fn without_a_time_the_excerpt_is_the_task_or_the_tail() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let log = "2026-10-21 21:07:01.100 [ERROR] mediaa_agent::worker: task 7 index (interview.mp4) failed: boom\n";
        assert!(log_excerpt(log, 7, None).contains("task 7"));
        let other = "2026-10-21 21:07:01.100 [INFO] mediaa: hello\n";
        assert!(log_excerpt(other, 9, None).contains("hello"));
    }
}

