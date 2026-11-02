#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_agent::outline::{PLAN_KIND, TOPICS_KIND};
    use mediaa_queue::Role;
    use serde_json::json;

    fn ready(replies: bool, speakers: bool, index: bool, topics: bool, details: bool) -> Readiness {
        Readiness {
            replies,
            speakers,
            index,
            topics,
            details,
        }
    }

    const ALL: Readiness = Readiness {
        replies: true,
        speakers: true,
        index: true,
        topics: true,
        details: true,
    };

    #[test]
    fn stages_are_offered_as_redo_or_add_by_the_data() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        assert_eq!(
            offer(Stage::Transcribe, &Readiness::default(), false),
            Offer::Redo,
            "the recognition is always offered"
        );
        for stage in [Stage::Diarize, Stage::Index, Stage::Topics, Stage::Details] {
            assert_eq!(
                offer(stage, &Readiness::default(), true),
                Offer::Unavailable(Why::NoReplies),
                "{stage:?} needs replies"
            );
            assert_eq!(offer(stage, &ALL, true), Offer::Redo, "{stage:?} exists");
        }
        let bare = ready(true, false, false, false, false);
        assert_eq!(offer(Stage::Diarize, &bare, true), Offer::Add);
        assert_eq!(offer(Stage::Index, &bare, true), Offer::Add);
        assert_eq!(
            offer(Stage::Topics, &bare, true),
            Offer::Unavailable(Why::NeedsIndex)
        );
        assert_eq!(
            offer(Stage::Details, &bare, true),
            Offer::Unavailable(Why::NeedsTopics)
        );
        let indexed = ready(true, false, true, false, false);
        assert_eq!(offer(Stage::Topics, &indexed, true), Offer::Add);
        let topics = ready(true, false, true, true, false);
        assert_eq!(offer(Stage::Details, &topics, true), Offer::Add);
        assert_eq!(
            offer(Stage::Diarize, &ALL, false),
            Offer::Unavailable(Why::NoModel),
            "no diarization model"
        );
    }

    #[test]
    fn while_an_analysis_runs_only_stop_is_usable() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let items = menu(&ALL, &[], true, true, None, true);
        assert_eq!(items[0].command, Command::Stop);
        assert!(items[0].enabled);
        assert!(items[1..].iter().all(|i| !i.enabled), "{items:?}");
        assert!(
            items[1..items.len() - 1]
                .iter()
                .all(|i| i.why == Some(Why::Busy) && i.live.is_none()),
            "{items:?}"
        );
        assert_eq!(items.len(), 1 + 1 + Stage::ALL.len() + 1);
        let idle = menu(&ALL, &[], false, true, None, true);
        assert!(idle.iter().all(|i| i.command != Command::Stop));
        assert!(idle.iter().all(|i| i.enabled && i.live.is_none()));
        assert_eq!(idle[0].command, Command::Analyse);
        let stages: Vec<Command> = idle
            .iter()
            .skip(1)
            .take(Stage::ALL.len())
            .map(|i| i.command)
            .collect();
        assert_eq!(stages, Stage::ALL.map(Command::Stage));
        assert_eq!(idle.last().unwrap().command, Command::Names);
        let bare = menu(
            &ready(true, false, false, false, false),
            &[],
            false,
            true,
            None,
            true,
        );
        let topics = bare
            .iter()
            .find(|i| i.command == Command::Stage(Stage::Topics))
            .unwrap();
        assert!(!topics.enabled);
        assert_eq!(topics.why, Some(Why::NeedsIndex));
        assert_eq!(topics.offer, Some(Offer::Unavailable(Why::NeedsIndex)));
    }

    #[test]
    fn the_names_item_follows_the_run() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let bare = menu(
            &ready(true, false, true, true, true),
            &[],
            false,
            true,
            None,
            true,
        );
        let names = bare.last().unwrap();
        assert_eq!(
            (names.command, names.enabled, names.why),
            (Command::Names, false, Some(Why::NoSpeakers))
        );
        let no_model = menu(&ALL, &[], false, true, None, false);
        assert_eq!(
            (
                no_model.last().unwrap().enabled,
                no_model.last().unwrap().why
            ),
            (false, Some(Why::NoNamesModel))
        );
        let idle = menu(&ALL, &[], false, true, None, true);
        assert_eq!(
            (
                idle.last().unwrap().command,
                idle.last().unwrap().enabled,
                idle.last().unwrap().why
            ),
            (Command::Names, true, None)
        );
        let mut running = task(5, "speaker-names", TaskStatus::Running, None);
        running.progress = 0.4;
        let items = menu(&ALL, &[], false, true, Some(&running), true);
        let names = &items[items.len() - 2];
        assert_eq!(
            (names.command, names.enabled, names.names),
            (
                Command::Names,
                false,
                Some(NamesLive::Running { percent: 40 })
            )
        );
        assert!(names_label(NamesLive::Running { percent: 40 }).contains("40"));
        assert_ne!(
            names_label(NamesLive::Queued),
            names_label(NamesLive::Paused)
        );
        let stop = items.last().unwrap();
        assert_eq!((stop.command, stop.enabled), (Command::StopNames, true));
        for (status, live) in [
            (TaskStatus::Queued, NamesLive::Queued),
            (TaskStatus::Paused, NamesLive::Paused),
        ] {
            let t = task(6, "speaker-names", status, None);
            let items = menu(&ALL, &[], false, true, Some(&t), true);
            let names = items.iter().find(|i| i.command == Command::Names).unwrap();
            assert_eq!(names.names, Some(live), "{status:?}");
        }
        for status in [TaskStatus::Done, TaskStatus::Failed, TaskStatus::Cancelled] {
            let t = task(7, "speaker-names", status, None);
            let items = menu(&ALL, &[], false, true, Some(&t), true);
            assert!(
                items.last().unwrap().enabled && items.last().unwrap().names.is_none(),
                "{status:?}"
            );
            assert!(
                items.iter().all(|i| i.command != Command::StopNames),
                "{status:?}"
            );
        }
    }

    fn stage_item(items: &[MenuItem], stage: Stage) -> &MenuItem {
        items
            .iter()
            .find(|item| item.command == Command::Stage(stage))
            .unwrap()
    }

    #[test]
    fn the_menu_reads_the_running_analysis() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let analysis = task(1, "analysis", TaskStatus::Running, None);
        let mut index = task(2, "index", TaskStatus::Running, Some(1));
        index.progress = 0.42;
        let mut topics = task(3, TOPICS_KIND, TaskStatus::Waiting, Some(1));
        topics.input = json!({ "options": { "details": true } });
        let flat = vec![
            analysis.clone(),
            index,
            topics,
            task(9, "transcribe", TaskStatus::Queued, Some(8)),
        ];
        let tree = descendants(&flat, 1);
        assert!(
            tree.iter().any(|item| item.id == 2) && tree.iter().all(|item| item.id != 9),
            "only this analysis"
        );
        let rows = super::rows(
            &analysis,
            &tree,
            &[],
            &ready(true, false, false, false, false),
            0,
        );
        let items = menu(
            &ready(true, false, false, false, false),
            &rows,
            true,
            true,
            None,
            true,
        );
        assert!(items[0].command == Command::Stop && items[0].enabled);
        assert!(
            !stage_item(&items, Stage::Transcribe).enabled
                && stage_item(&items, Stage::Transcribe).live.is_none()
        );
        assert_eq!(
            stage_item(&items, Stage::Transcribe).offer,
            Some(Offer::Redo),
            "the replies are already there"
        );
        let index = stage_item(&items, Stage::Index);
        assert_eq!(
            index.live,
            Some(Live::Running {
                percent: 42,
                count: None
            })
        );
        assert!(
            index.offer.is_none() && !index.enabled,
            "running is not «Добавить»"
        );
        assert!(live_label(Stage::Index, index.live.unwrap()).contains("42%"));
        assert_eq!(
            stage_item(&items, Stage::Diarize).offer,
            Some(Offer::Add),
            "left out of this run"
        );
        assert!(stage_item(&items, Stage::Diarize).live.is_none());
        assert_eq!(stage_item(&items, Stage::Topics).live, Some(Live::Queued));
        assert_eq!(
            stage_item(&items, Stage::Details).live,
            Some(Live::Queued),
            "the topics will add the breakdown"
        );
        let queued = live_label(Stage::Topics, Live::Queued);
        let added = menu_label(Stage::Topics, Some(Offer::Add));
        assert_ne!(queued, added);
        assert!(!queued.contains(&added));

        let row = |stage, state| StageRow {
            stage,
            state,
            progress: 0.0,
            elapsed: None,
            left: None,
            count: None,
            task: None,
        };
        let rows = [
            row(Stage::Diarize, RowState::Paused),
            row(Stage::Topics, RowState::Failed("no model".into())),
            row(Stage::Details, RowState::Cancelled),
        ];
        let items = menu(&bare_ready(), &rows, true, true, None, true);
        assert_eq!(stage_item(&items, Stage::Diarize).live, Some(Live::Paused));
        assert_eq!(stage_item(&items, Stage::Topics).live, Some(Live::Failed));
        assert_eq!(
            stage_item(&items, Stage::Details).live,
            Some(Live::Cancelled)
        );
        for stage in [Stage::Diarize, Stage::Topics, Stage::Details] {
            let item = stage_item(&items, stage);
            assert!(
                item.offer.is_none() && !item.enabled && item.why.is_none(),
                "{stage:?} is the run, not «Добавить»"
            );
        }
    }

    fn bare_ready() -> Readiness {
        ready(true, false, false, false, false)
    }

    #[test]
    fn a_restart_rebuilds_the_stage_and_every_later_enabled_one() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let defaults = AnalysisOptions::default();
        let p = plan(Stage::Index, &ALL, &defaults, true, None);
        assert_eq!(p.stages, [Stage::Index, Stage::Topics, Stage::Details]);
        let p = plan(Stage::Transcribe, &ALL, &defaults, true, None);
        assert_eq!(p.stages, Stage::ALL);
        let p = plan(Stage::Details, &ALL, &defaults, true, None);
        assert_eq!(p.stages, [Stage::Details]);
        let p = plan(Stage::Transcribe, &ALL, &defaults, false, None);
        assert_eq!(
            p.stages,
            [
                Stage::Transcribe,
                Stage::Index,
                Stage::Topics,
                Stage::Details
            ]
        );
        let off = AnalysisOptions {
            diarize: false,
            speakers: None,
            index: true,
            topics: false,
            details: false,
        };
        let bare = ready(true, false, true, false, false);
        assert_eq!(
            plan(Stage::Index, &bare, &off, true, None).stages,
            [Stage::Index]
        );
        assert_eq!(
            plan(Stage::Topics, &bare, &defaults, true, None).stages,
            [Stage::Topics, Stage::Details]
        );
        let odd = AnalysisOptions {
            topics: false,
            details: true,
            ..defaults
        };
        assert_eq!(
            plan(Stage::Index, &bare, &odd, true, None).stages,
            [Stage::Index]
        );
        let p = plan(Stage::Diarize, &bare, &defaults, true, Some(3));
        assert_eq!(p.options.speakers, Some(3));
        assert_eq!(p.stages[0], Stage::Diarize);
        let p = plan(
            Stage::Topics,
            &ready(true, false, true, true, true),
            &off,
            true,
            None,
        );
        assert!(p.options.index && p.options.topics);
        assert_eq!(p.stages, [Stage::Topics, Stage::Details]);
    }

    #[test]
    fn the_confirmation_lists_what_is_rebuilt() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let p = plan(Stage::Index, &ALL, &AnalysisOptions::default(), true, None);
        let text = confirm_text(&p, false);
        for stage in [Stage::Index, Stage::Topics, Stage::Details] {
            assert!(text.contains(&stage_name(stage).to_lowercase()), "{text}");
        }
        assert!(
            !text.contains(&stage_name(Stage::Transcribe).to_lowercase()),
            "{text}"
        );
        assert_ne!(confirm_text(&p, true), text, "adding is worded differently");
    }

    #[test]
    fn the_options_keep_their_dependencies() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let mut draft = Draft::new(&AnalysisOptions::default(), true);
        assert!(!draft.diarize && draft.index && draft.topics && draft.details);
        draft.index = false;
        draft.normalize();
        assert!(!draft.topics && !draft.details && !draft.can_topics() && !draft.can_details());
        draft.index = true;
        draft.normalize();
        assert!(
            draft.can_topics() && !draft.can_details(),
            "the topics stay off until chosen again"
        );
        draft.topics = true;
        assert!(draft.can_details());
        let draft = Draft::new(&AnalysisOptions::default(), false);
        assert!(!draft.diarize && !draft.can_diarize());
        assert!(!draft.options().diarize);
        let no_model = mediaa_core::agent::ModelChoices::default();
        assert!(!draft.options().resolved(&no_model).diarize);
        let mut draft = Draft::new(&AnalysisOptions::default(), true);
        draft.diarize = true;
        draft.speakers = " 3 ".into();
        assert_eq!(draft.options().speakers, Some(3));
        draft.speakers = "abc".into();
        assert_eq!(draft.options().speakers, None);
        draft.speakers = "0".into();
        assert_eq!(draft.options().speakers, None);
        draft.speakers = "2".into();
        draft.diarize = false;
        assert_eq!(draft.options().speakers, None);
        let saved = AnalysisOptions {
            diarize: true,
            speakers: Some(4),
            index: true,
            topics: false,
            details: false,
        };
        assert_eq!(Draft::new(&saved, true).options(), saved);
    }

    #[test]
    fn availability_follows_the_data() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let none = availability(&Readiness::default(), None);
        assert_eq!(none.pane, Pane::Start);
        assert!(!none.chat && !none.search && !none.topics && !none.compact);
        for status in [
            TaskStatus::Queued,
            TaskStatus::Running,
            TaskStatus::Waiting,
            TaskStatus::Paused,
            TaskStatus::Failed,
            TaskStatus::Cancelled,
        ] {
            assert_eq!(
                availability(&Readiness::default(), Some(status)).pane,
                Pane::Progress,
                "{status:?}"
            );
        }
        assert_eq!(
            availability(&Readiness::default(), Some(TaskStatus::Done)).pane,
            Pane::NoSpeech
        );
        let replies = ready(true, false, false, false, false);
        let running = availability(&replies, Some(TaskStatus::Running));
        assert_eq!(running.pane, Pane::Replies);
        assert!(running.chat && running.compact && !running.search && !running.topics);
        assert!(
            availability(&replies, Some(TaskStatus::Failed)).compact,
            "a failure stays visible"
        );
        assert!(!availability(&replies, Some(TaskStatus::Done)).compact);
        assert!(!availability(&replies, Some(TaskStatus::Cancelled)).compact);
        assert!(!availability(&replies, None).compact);
        let indexed = availability(&ready(true, false, true, false, false), None);
        assert!(indexed.search && !indexed.topics);
        let topics = availability(&ready(true, false, true, true, false), None);
        assert!(topics.topics);
        assert!(
            !availability(
                &ready(false, false, false, true, true),
                Some(TaskStatus::Running)
            )
            .topics
        );
    }

    #[test]
    fn time_is_said_in_minutes() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        assert_eq!(left(None), Left::Unknown);
        assert_eq!(left(Some(Duration::ZERO)), Left::Under);
        assert_eq!(left(Some(Duration::from_secs(59))), Left::Under);
        assert_eq!(left(Some(Duration::from_secs(60))), Left::Minutes(1));
        assert_eq!(left(Some(Duration::from_secs(89))), Left::Minutes(1));
        assert_eq!(left(Some(Duration::from_secs(90))), Left::Minutes(2));
        assert_eq!(
            left(Some(Duration::from_secs(4 * 60 + 10))),
            Left::Minutes(4)
        );
        let texts = [
            left_text(Left::Unknown),
            left_text(Left::Under),
            left_text(Left::Minutes(4)),
        ];
        assert!(texts[2].contains('4'));
        assert!(texts[0] != texts[1] && texts[1] != texts[2]);
        assert_eq!(clock(Duration::from_secs(0)), "0:00");
        assert_eq!(clock(Duration::from_secs(83)), "1:23");
        assert_eq!(clock(Duration::from_secs(12 * 60 + 5)), "12:05");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
    }

    fn task(id: i64, kind: &str, status: TaskStatus, parent: Option<i64>) -> Task {
        Task {
            id,
            kind: kind.into(),
            title: kind.into(),
            input: serde_json::Value::Null,
            priority: 0,
            status,
            cursor: serde_json::Value::Null,
            partial: serde_json::Value::Null,
            result: None,
            progress: 0.0,
            error: None,
            note: None,
            attempts: 0,
            created_ms: 0,
            updated_ms: 0,
            parent,
            scope: Some("f".into()),
            inline: false,
            started_ms: None,
            finished_ms: None,
            stats: Default::default(),
            detail: serde_json::Value::Null,
            units: 0.0,
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

    fn state_of(rows: &[StageRow], stage: Stage) -> RowState {
        rows.iter()
            .find(|r| r.stage == stage)
            .unwrap()
            .state
            .clone()
    }

    #[test]
    fn rows_follow_the_tasks_of_the_analysis() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let analysis = task(1, "analysis", TaskStatus::Running, None);
        let mut transcribe = task(2, "transcribe", TaskStatus::Running, Some(1));
        transcribe.progress = 0.4;
        transcribe.started_ms = Some(1_000);
        transcribe.units = 10.0;
        let index = task(3, "index", TaskStatus::Queued, Some(1));
        let mut topics = task(4, TOPICS_KIND, TaskStatus::Waiting, Some(1));
        topics.input = json!({ "options": { "details": true } });
        let tree = vec![analysis.clone(), transcribe, index, topics];
        let stats = [KindStat {
            kind: "transcribe".into(),
            runs: 1,
            ms: 60_000,
            units: 10.0,
        }];
        let rows = super::rows(&analysis, &tree, &stats, &Readiness::default(), 31_000);
        assert_eq!(rows.iter().map(|r| r.stage).collect::<Vec<_>>(), Stage::ALL);
        let first = &rows[0];
        assert_eq!(first.state, RowState::Running);
        assert_eq!(first.elapsed, Some(Duration::from_secs(30)));
        let left_s = first.left.unwrap().as_secs_f64();
        assert!((left_s - 45.0).abs() < 0.01, "30 s at 40 %: {left_s}");
        assert_eq!(
            state_of(&rows, Stage::Diarize),
            RowState::Off,
            "not part of this analysis"
        );
        assert_eq!(state_of(&rows, Stage::Index), RowState::Waiting);
        assert_eq!(state_of(&rows, Stage::Topics), RowState::Waiting);
        assert_eq!(
            state_of(&rows, Stage::Details),
            RowState::Waiting,
            "the merge will add them"
        );
        assert_eq!(current(&rows).unwrap().stage, Stage::Transcribe);
    }

    #[test]
    fn rows_of_earlier_data_failures_and_counts() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let analysis = task(1, "analysis", TaskStatus::Running, None);
        let index = task(2, "index", TaskStatus::Done, Some(1));
        let topics = task(3, TOPICS_KIND, TaskStatus::Running, Some(1));
        let plan_task = task(4, PLAN_KIND, TaskStatus::Done, Some(3));
        let tree = vec![
            analysis.clone(),
            index,
            topics,
            plan_task,
            task(5, DIGEST_KIND, TaskStatus::Done, Some(3)),
            task(6, DIGEST_KIND, TaskStatus::Done, Some(3)),
            task(7, DIGEST_KIND, TaskStatus::Running, Some(3)),
        ];
        let have = ready(true, true, true, false, false);
        let rows = super::rows(&analysis, &tree, &[], &have, 0);
        assert_eq!(state_of(&rows, Stage::Transcribe), RowState::Done);
        assert_eq!(state_of(&rows, Stage::Diarize), RowState::Done);
        assert_eq!(state_of(&rows, Stage::Index), RowState::Done);
        assert_eq!(state_of(&rows, Stage::Details), RowState::Off);
        let topics = rows.iter().find(|r| r.stage == Stage::Topics).unwrap();
        assert_eq!(
            (topics.state.clone(), topics.count),
            (RowState::Running, Some((2, 3)))
        );
        assert!(
            row_name(topics).contains("2") && row_name(topics).contains('3'),
            "{}",
            row_name(topics)
        );
        assert_eq!(
            compact_title(TaskStatus::Running, &rows, Left::Minutes(4))
                .matches('4')
                .count(),
            1
        );
        assert!(compact_title(TaskStatus::Running, &rows, Left::Minutes(4))
            .contains(&stage_name(Stage::Topics).to_lowercase()));

        let failed_analysis = task(1, "analysis", TaskStatus::Failed, None);
        let mut index = task(2, "index", TaskStatus::Failed, Some(1));
        index.error = Some("нет модели".into());
        let topics = task(3, TOPICS_KIND, TaskStatus::Queued, Some(1));
        let tree = vec![failed_analysis.clone(), index, topics];
        let rows = super::rows(
            &failed_analysis,
            &tree,
            &[],
            &ready(true, false, false, false, false),
            0,
        );
        assert_eq!(
            state_of(&rows, Stage::Index),
            RowState::Failed("нет модели".into())
        );
        assert_eq!(
            rows.iter().find(|r| r.stage == Stage::Index).unwrap().task,
            Some(2),
            "the retry resumes this task"
        );
        assert_eq!(state_of(&rows, Stage::Topics), RowState::Off);
        assert!(compact_title(TaskStatus::Failed, &rows, Left::Unknown)
            .contains(&stage_name(Stage::Index).to_lowercase()));
    }

    #[test]
    fn row_texts_for_each_state() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let row = |state: RowState, elapsed: Option<u64>, left: Option<u64>| StageRow {
            stage: Stage::Index,
            state,
            progress: 0.5,
            elapsed: elapsed.map(Duration::from_secs),
            left: left.map(Duration::from_secs),
            count: None,
            task: None,
        };
        let running = row_time_text(&row(RowState::Running, Some(83), Some(240)));
        assert!(
            running.contains("1:23") && running.contains('4'),
            "{running}"
        );
        let no_estimate = row_time_text(&row(RowState::Running, Some(5), None));
        assert!(
            no_estimate.contains("0:05") && !no_estimate.contains('·'),
            "{no_estimate}"
        );
        let short = row_time_text(&row(RowState::Running, Some(70), Some(20)));
        assert!(short.contains(&left_text(Left::Under)), "{short}");
        assert_eq!(row_time_text(&row(RowState::Done, Some(125), None)), "2:05");
        assert_eq!(row_time_text(&row(RowState::Done, None, None)), "");
        assert_eq!(
            row_time_text(&row(RowState::Failed("x".into()), Some(1), None)),
            "",
            "the failed row shows the button"
        );
        let texts: Vec<String> = [
            RowState::Waiting,
            RowState::Paused,
            RowState::Cancelled,
            RowState::Off,
        ]
        .into_iter()
        .map(|s| row_time_text(&row(s, None, None)))
        .collect();
        for (i, a) in texts.iter().enumerate() {
            assert!(!a.is_empty());
            for b in &texts[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(
            total_title(TaskStatus::Running, Left::Minutes(4)),
            total_title(TaskStatus::Queued, Left::Minutes(4))
        );
        assert!(total_title(TaskStatus::Running, Left::Minutes(4)).contains('4'));
        assert_ne!(
            total_title(TaskStatus::Failed, Left::Unknown),
            total_title(TaskStatus::Cancelled, Left::Unknown)
        );
    }

    #[test]
    fn the_row_columns_share_the_header_geometry() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let rect = Rect::from_min_size(
            egui::pos2(100.0, 200.0),
            egui::vec2(480.0, topics::HEADER_H),
        );
        let h = topics::header(rect);
        let g = row_geom(rect);
        assert_eq!(g.icon, h.icon, "the icon is the header's icon");
        assert_eq!(
            g.name.left(),
            h.text.left(),
            "the name starts where the header's text does"
        );
        assert_eq!(
            g.time.right(),
            h.caret.right(),
            "the right text ends at the caret column"
        );
        assert_eq!(g.bar.center().y, rect.center().y);
        assert!((g.bar.left() - g.name.right() - COL_GAP).abs() < 1e-6);
        assert!((g.time.left() - g.bar.right() - COL_GAP).abs() < 1e-6);
        assert_eq!((g.bar.width(), g.time.width()), (BAR_W, TIME_W));
        let narrow = row_geom(Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(120.0, topics::HEADER_H),
        ));
        for r in [narrow.name, narrow.bar, narrow.time] {
            assert!(r.width() >= 0.0, "{r:?}");
        }
    }

    #[test]
    fn a_running_stage_leads_the_compact_line_and_an_empty_error_has_no_bare_dash() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let running = StageRow {
            stage: Stage::Topics,
            state: RowState::Running,
            progress: 0.4,
            elapsed: Some(Duration::from_secs(10)),
            left: Some(Duration::from_secs(240)),
            count: Some((2, 5)),
            task: Some(3),
        };
        let title = compact_title(
            TaskStatus::Failed,
            std::slice::from_ref(&running),
            Left::Minutes(4),
        );
        assert_eq!(
            title,
            compact_title(
                TaskStatus::Running,
                std::slice::from_ref(&running),
                Left::Minutes(4)
            )
        );
        assert!(
            shows_spinner(TaskStatus::Failed, std::slice::from_ref(&running)),
            "a restarted stage spins"
        );

        let failed = StageRow {
            stage: Stage::Index,
            state: RowState::Failed("  ".into()),
            progress: 0.0,
            elapsed: None,
            left: None,
            count: None,
            task: Some(2),
        };
        let title = compact_title(TaskStatus::Failed, &[failed], Left::Unknown);
        let bare = t!("analysis.compact_failed", stage = "").to_string();
        assert_ne!(title, bare);
        assert!(!title.trim_end().ends_with('—'), "{title}");
        assert!(
            title.contains(&stage_name(Stage::Index).to_lowercase()),
            "{title}"
        );

        let explained = StageRow {
            stage: Stage::Index,
            state: RowState::Failed("нет модели".into()),
            progress: 0.0,
            elapsed: None,
            left: None,
            count: None,
            task: Some(2),
        };
        let title = compact_title(TaskStatus::Failed, &[explained], Left::Unknown);
        assert!(
            title.contains("нет модели")
                || title.contains(&stage_name(Stage::Index).to_lowercase()),
            "{title}"
        );
        assert_eq!(
            compact_title(TaskStatus::Failed, &[], Left::Unknown),
            t!("analysis.total_failed").to_string()
        );
        assert_eq!(short_reason("  \nboom\n"), "boom");
        assert!(short_reason(&"x".repeat(200)).ends_with('…'));
        assert!(!shows_spinner(TaskStatus::Failed, &[]));
        assert!(shows_spinner(TaskStatus::Queued, &[]));
    }

    #[test]
    fn topics_and_the_breakdown_count_as_running() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let digest = task(5, DIGEST_KIND, TaskStatus::Running, Some(3));
        assert!(topics_running(&[digest]));
        assert!(!topics_running(&[task(
            2,
            "index",
            TaskStatus::Running,
            None
        )]));
        assert!(!topics_running(&[task(
            6,
            DETAIL_KIND,
            TaskStatus::Waiting,
            None
        )]));
        assert!(topics_running(&[task(
            7,
            DETAIL_KIND,
            TaskStatus::Running,
            None
        )]));
    }
}

