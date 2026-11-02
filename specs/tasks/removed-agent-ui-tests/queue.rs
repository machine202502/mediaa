#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn snapshot_of_the_task_details_modal() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let (store, _sched, _root, _kids, _clock) = tree();
        let task = store
            .top_level(&Filter::default())
            .unwrap()
            .into_iter()
            .next()
            .expect("a queued task");
        let now = task.started_ms.unwrap_or_default() + 5_000;
        let copied = false;
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, copied: &mut bool| {
                task_details::show(
                    ui.ctx(),
                    &task,
                    &Stats::default(),
                    now,
                    "log line",
                    copied,
                    &crate::ui::pages::agent::mentions::Roster::default(),
                );
            },
            copied,
        );
        crate::ui::theme::install(&harness.ctx);
        harness.run_steps(2);
        harness.snapshot("queue-task-details");
    }
    #[test]
    fn the_load_lists_pools_and_the_web_tool() {
        let state = slots::Status {
            groups: vec![
                slots::GroupLoad {
                    name: "Single".into(),
                    busy: 1,
                    limit: 2,
                    waiting: 0,
                },
                slots::GroupLoad {
                    name: "Local".into(),
                    busy: 0,
                    limit: 1,
                    waiting: 2,
                },
            ],
            web: slots::ToolLoad {
                busy: 2,
                limit: 2,
                waiting: 1,
            },
        };
        let s = rows(&state);
        assert_eq!(
            s,
            [
                ("Single".to_string(), 1, 2, 0),
                ("Local".to_string(), 0, 1, 2)
            ]
        );
        assert_eq!(
            (state.web.busy, state.web.limit, state.web.waiting),
            (2, 2, 1)
        );
    }

    use mediaa_queue::{FakeClock, MemStore, NewTask, Resource};
    use serde_json::json;
    use std::sync::Arc;

    type Tree = (
        Arc<dyn Store>,
        Arc<Scheduler>,
        TaskId,
        Vec<TaskId>,
        Arc<FakeClock>,
    );

    fn tree() -> Tree {
        let clock = Arc::new(FakeClock::new(0));
        let store: Arc<dyn Store> = Arc::new(MemStore::new(clock.clone()));
        let sched = Arc::new(Scheduler::new(store.clone(), vec![]));
        let root = store
            .enqueue(
                NewTask::new("analysis", "talk.mp4", &json!({}))
                    .composite()
                    .scope("rec-1"),
            )
            .unwrap();
        let mut kids = Vec::new();
        for kind in ["transcribe", "index", "topics"] {
            kids.push(
                store
                    .enqueue(
                        NewTask::new(kind, "talk.mp4", &json!({}))
                            .resource(Resource::Llm)
                            .parent(root)
                            .scope("rec-1")
                            .units(2.0),
                    )
                    .unwrap(),
            );
        }
        (store, sched, root, kids, clock)
    }

    #[test]
    fn rows_flatten_in_order_and_open_on_demand() {
        let (store, _sched, root, kids, clock) = tree();
        clock.advance(1_000);
        let chat = store
            .begin_inline("chat", "Когда?", &json!({}), None, Some("rec-2"))
            .unwrap();
        let tool = store
            .begin_inline("tool", "Ищу", &json!({}), Some(chat), Some("rec-2"))
            .unwrap();
        let snap = read(store.as_ref(), &Filter::default()).unwrap();
        let mut open = HashSet::new();
        let ids = |rows: &[Row]| {
            rows.iter()
                .map(|r| (snap.tasks[r.index].id, r.depth))
                .collect::<Vec<_>>()
        };
        let rows = flatten(&snap.tasks, &snap.tops, &open);
        assert_eq!(
            ids(&rows),
            [(chat, 0), (root, 0)],
            "closed: top-level tasks only, running first"
        );
        assert_eq!(
            (rows[0].children, rows[1].children, rows[1].open),
            (1, 3, false)
        );
        open.insert(root);
        open.insert(chat);
        let rows = flatten(&snap.tasks, &snap.tops, &open);
        assert_eq!(
            ids(&rows),
            [
                (chat, 0),
                (tool, 1),
                (root, 0),
                (kids[0], 1),
                (kids[1], 1),
                (kids[2], 1)
            ]
        );
        assert!(rows[0].open && rows[2].open && !rows[1].open);
    }

    #[test]
    fn filters_pick_the_group_and_the_recording() {
        let (store, _sched, root, _, _clock) = tree();
        let chat = store
            .begin_inline("chat", "Когда?", &json!({}), None, Some("rec-2"))
            .unwrap();
        let tops = |f: &Filter| read(store.as_ref(), f).unwrap().tops;
        assert_eq!(tops(&Filter::group(KindGroup::Analysis)), [root]);
        assert_eq!(tops(&Filter::group(KindGroup::Chat)), [chat]);
        assert_eq!(tops(&Filter::scope("rec-2")), [chat]);
        let snap = read(store.as_ref(), &Filter::group(KindGroup::Chat)).unwrap();
        assert_eq!(snap.tasks.len(), 1, "only the tasks of the filter are read");
        assert_eq!(
            snap.scopes,
            [
                ("rec-2".to_string(), "rec-2…".to_string()),
                ("rec-1".to_string(), "talk.mp4".to_string())
            ],
            "the scopes are not narrowed by the filter; by name"
        );
    }

    fn task(kind: &str, status: TaskStatus) -> Task {
        let (store, ..) = tree();
        let id = store
            .enqueue(NewTask::new(kind, "x", &json!({})).resource(Resource::Llm))
            .unwrap();
        let mut t = store.get(id).unwrap().unwrap();
        t.status = status;
        t
    }

    #[test]
    fn the_time_column_says_elapsed_left_or_the_duration() {
        let mut t = task("transcribe", TaskStatus::Running);
        t.started_ms = Some(1_000);
        let running = time_text(
            &t,
            Duration::from_secs(83),
            Some(Duration::from_secs(250)),
            true,
        );
        assert!(
            running.starts_with(&t!("analysis.elapsed", time = "1:23").to_string())
                && running.contains('4'),
            "{running}"
        );
        assert!(
            time_text(&t, Duration::from_secs(5), None, true)
                .ends_with(&t!("analysis.left_unknown").to_string()),
            "no estimate yet"
        );
        assert!(time_text(
            &t,
            Duration::from_secs(5),
            Some(Duration::from_secs(20)),
            true
        )
        .ends_with(&t!("analysis.left_under").to_string()));
        assert_eq!(
            time_text(
                &t,
                Duration::from_secs(5),
                Some(Duration::from_secs(20)),
                false
            ),
            t!("analysis.elapsed", time = "0:05").to_string(),
            "chat: no estimate"
        );
        t.status = TaskStatus::Done;
        assert_eq!(
            time_text(&t, Duration::from_secs(754), Some(Duration::ZERO), true),
            "12:34"
        );
        t.status = TaskStatus::Queued;
        t.started_ms = None;
        assert_eq!(
            time_text(&t, Duration::ZERO, Some(Duration::from_secs(600)), true),
            left_text(left(Some(Duration::from_secs(600))))
        );
        t.status = TaskStatus::Cancelled;
        assert_eq!(
            time_text(&t, Duration::ZERO, None, true),
            "",
            "never started"
        );
        t.status = TaskStatus::Failed;
        t.started_ms = Some(1_000);
        assert_eq!(
            time_text(&t, Duration::from_secs(3), None, true),
            "0:03",
            "a failure shows when it started"
        );
        t.status = TaskStatus::Stopping;
        assert!(
            time_text(&t, Duration::from_secs(5), None, true).contains("0:05"),
            "stopping keeps the clock"
        );
    }

    #[test]
    fn a_composite_counts_its_children_and_sums_their_work() {
        let (store, _sched, root, kids, _clock) = tree();
        store.claim(kids[0]).unwrap();
        store
            .add_stats(
                kids[0],
                &Stats {
                    model_calls: 2,
                    prompt_tokens: 100,
                    completion_tokens: 20,
                    model_ms: 5,
                    models: vec!["C:\\m\\Qwen3-Q4.gguf#4096".into()],
                },
            )
            .unwrap();
        store
            .add_stats(
                kids[1],
                &Stats {
                    model_calls: 1,
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    model_ms: 1,
                    models: vec!["owner/embed".into()],
                },
            )
            .unwrap();
        store.complete(kids[0], &json!({})).unwrap();
        let snap = read(store.as_ref(), &Filter::default()).unwrap();
        let top = snap.tasks.iter().find(|t| t.id == root).unwrap();
        assert_eq!(
            count_text(top, &snap.tasks),
            Some(t!("analysis.of", done = 1, total = 3).to_string())
        );
        let leaf = snap.tasks.iter().find(|t| t.id == kids[2]).unwrap();
        assert_eq!(count_text(leaf, &snap.tasks), None, "a leaf has no count");
        let stats = subtree_stats(top, &snap.tasks);
        assert_eq!(
            (
                stats.model_calls,
                stats.prompt_tokens,
                stats.completion_tokens
            ),
            (3, 110, 25)
        );
        let text = stats_text(&stats);
        assert!(
            text.contains("110")
                && text.contains("25")
                && text.contains("Qwen3-Q4.gguf")
                && text.contains("embed")
                && !text.contains("owner"),
            "{text}"
        );
        assert_eq!(stats_text(&Stats::default()), "", "nothing called");
    }

    #[test]
    fn a_tool_row_shows_the_tool_its_arguments_and_the_size_of_the_result() {
        assert_eq!(tool_text(&json!({})), None);
        let detail =
            json!({ "tool": "search", "args": "{\"query\":\"отчёт\"}", "result_chars": 1234 });
        let text = tool_text(&detail).unwrap();
        assert!(
            text.starts_with("search {\"query\"")
                && text.ends_with(&t!("queue.tool_result", n = 1234).to_string()),
            "{text}"
        );
        let mut tool = task("tool", TaskStatus::Done);
        tool.detail = detail;
        tool.stats = Stats {
            model_calls: 1,
            prompt_tokens: 7,
            completion_tokens: 0,
            model_ms: 1,
            models: vec!["embed".into()],
        };
        let stats = stats_text(&tool.stats);
        assert!(stats.contains('7') && stats.ends_with("embed"), "{stats}");
    }

    #[test]
    fn the_row_says_the_label_and_the_error() {
        let mut t = task("topic-digest", TaskStatus::Failed);
        t.title = "#3".into();
        t.error = Some("  модель упала \n".into());
        let texts = row_texts(&t, std::slice::from_ref(&t), &[], 0, "ru");
        assert_eq!(texts.label, task_label::label("topic-digest", "#3"));
        assert_eq!(texts.error.as_deref(), Some("модель упала"));
        t.note = Some(queue::pause_note("Индекс: rec"));
        assert_eq!(
            row_texts(&t, std::slice::from_ref(&t), &[], 0, "ru")
                .error
                .as_deref(),
            Some("ждёт: Индекс: rec на паузе")
        );
        assert_eq!(
            row_texts(&t, std::slice::from_ref(&t), &[], 0, "en")
                .error
                .as_deref(),
            Some("waiting: Индекс: rec is paused")
        );
        t.note = None;
        t.error = Some("   ".into());
        assert_eq!(
            row_texts(&t, std::slice::from_ref(&t), &[], 0, "ru").error,
            None
        );
    }

    #[test]
    fn a_note_is_words_in_the_language_of_the_window() {
        let failed = queue::failure_note("Темы: rec");
        assert_eq!(note_text(Some(&failed), "ru"), "из-за ошибки в Темы: rec");
        assert_eq!(
            note_text(Some(&failed), "en"),
            "because of a failure in Темы: rec"
        );
        let hang = queue::hang_note(queue::Resource::Llm, Duration::from_secs(300));
        assert_eq!(
            note_text(Some(&hang), "ru"),
            "модель не отвечает дольше 5 мин — вызов прерван, задача продолжит с последней точки"
        );
        assert_eq!(
            note_text(Some(&hang), "en"),
            "the model has not responded for over 5 min — the call was interrupted; the task will continue from the last point"
        );
        let short = queue::hang_note(queue::Resource::Embed, Duration::from_secs(45));
        assert_eq!(
            note_text(Some(&short), "ru"),
            "индексация не отвечает дольше 45 с — вызов прерван, задача продолжит с последней точки"
        );
        assert_eq!(note_text(None, "ru"), "");
    }

    #[test]
    fn actions_follow_the_state_and_the_kind_of_task() {
        let a = |kind: &str, status| actions(&task(kind, status));
        assert_eq!(
            a("transcribe", TaskStatus::Running),
            Actions {
                pause: true,
                cancel: true,
                remove: true,
                ..Actions::default()
            }
        );
        let failed = a("index", TaskStatus::Failed);
        assert!(failed.restart && !failed.resume && !failed.pause && !failed.cancel);
        let done = a("index", TaskStatus::Done);
        assert!(done.remove && !done.restart && !done.pause && !done.resume && !done.cancel);
        let paused = a("index", TaskStatus::Paused);
        assert!(paused.resume && paused.cancel && !paused.restart);
        assert!(a("index", TaskStatus::Cancelled).restart);
        assert!(!a("index", TaskStatus::Queued).restart);
        let mut chat = task("chat", TaskStatus::Running);
        chat.inline = true;
        assert_eq!(actions(&chat), Actions::default());
        chat.status = TaskStatus::Done;
        assert_eq!(
            actions(&chat),
            Actions {
                remove: true,
                ..Actions::default()
            }
        );
        let mut answer = task("chat-answer", TaskStatus::Running);
        answer.inline = false;
        let offered = actions(&answer);
        assert!(
            offered.cancel
                && !offered.pause
                && !offered.resume
                && !offered.restart
                && !offered.remove
        );
        answer.status = TaskStatus::Stopping;
        assert!(actions(&answer).cancel && !actions(&answer).resume);
        answer.status = TaskStatus::Done;
        assert_eq!(
            actions(&answer),
            Actions {
                remove: true,
                ..Actions::default()
            }
        );
        let mut child = task("index", TaskStatus::Done);
        child.parent = Some(1);
        assert!(!actions(&child).remove);
    }

    #[test]
    fn the_actions_of_a_composite_reach_its_subtree() {
        let (store, sched, root, kids, _clock) = tree();
        run(&sched, &Op::Pause(kids[0])).unwrap();
        assert_eq!(
            store.get(kids[0]).unwrap().unwrap().status,
            TaskStatus::Paused
        );
        assert_eq!(
            store.get(kids[1]).unwrap().unwrap().status,
            TaskStatus::Queued
        );
        assert_eq!(
            store.get(root).unwrap().unwrap().status,
            TaskStatus::Running
        );
        run(&sched, &Op::Resume(root)).unwrap();
        assert_eq!(
            store.get(kids[0]).unwrap().unwrap().status,
            TaskStatus::Queued
        );
        run(&sched, &Op::Cancel(root)).unwrap();
        for id in kids.iter().chain([&root]) {
            assert_eq!(
                store.get(*id).unwrap().unwrap().status,
                TaskStatus::Cancelled
            );
        }
        run(&sched, &Op::Restart(root)).unwrap();
        assert_eq!(
            store.get(kids[2]).unwrap().unwrap().status,
            TaskStatus::Queued
        );
        assert_eq!(
            store.get(root).unwrap().unwrap().status,
            TaskStatus::Running,
            "queued children are free to run"
        );
        run(&sched, &Op::Remove(root)).unwrap();
        assert!(store.list().unwrap().is_empty(), "the children go with it");
    }

    #[test]
    fn the_columns_stay_put_whatever_the_depth() {
        let row = Rect::from_min_size(egui::pos2(10.0, 100.0), egui::vec2(1000.0, ROW_H));
        let (a, b) = (geom(row, 0), geom(row, 3));
        assert_eq!(
            (a.bar, a.count, a.time, a.actions),
            (b.bar, b.count, b.time, b.actions)
        );
        assert_eq!(b.chevron.left() - a.chevron.left(), 3.0 * INDENT);
        assert!(
            a.label.right() <= a.bar.left()
                && a.bar.right() < a.count.left()
                && a.count.right() < a.time.left()
                && a.time.right() < a.actions[0].left()
        );
        assert_eq!(a.count.left() - a.bar.right(), GAP);
        assert_eq!(a.time.left() - a.count.right(), GAP);
        assert_eq!(a.actions[0].left() - a.time.right(), GAP);
        let narrow = geom(
            Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, ROW_H)),
            5,
        );
        assert!(narrow.label.width() >= 0.0);
    }

    #[test]
    fn a_click_opens_details_and_only_the_chevron_folds() {
        assert_eq!(row_intent(true, true), RowIntent::Fold);
        assert_eq!(row_intent(true, false), RowIntent::Details);
        assert_eq!(
            row_intent(false, true),
            RowIntent::Details,
            "a leaf has no chevron"
        );
        assert_eq!(row_intent(false, false), RowIntent::Details);
    }
}

