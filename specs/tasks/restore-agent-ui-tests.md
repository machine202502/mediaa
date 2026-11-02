# TST: восстановление удалённых UI-тестов агента

## Статус

`ожидание`. Работа не начата; план и исходники удалённых тестов сохранены
для следующего агента.

## Цель

Вернуть покрытие агентского UI, удалённое при переходе на protocol и
удалении `AgentView`/`AgentServices`: **54 старых теста** из пяти модулей.
Целевой минимум — вернуться к baseline **386 passed**, не добавляя во
frontend `mediaa-backend/storage/agent/engine/queue` и не нарушая границы
`ui/app/features/infra`.

## Исходники

Удалённые тест-модули лежат рядом:

```text
specs/tasks/removed-agent-ui-tests/
  README.md
  queue.rs          13 тестов
  analysis.rs       15 тестов
  task_details.rs   13 тестов
  dialog.rs          5 тестов
  chat.rs            8 тестов
```

Они извлечены из commit `d4401ff49d33e483bd05097a31546b9a30f4472c`,
файлы `crates/app/src/ui/pages/agent/*.rs`, только блоки
`#[cfg(test)] mod tests { ... }`. Код в них **не адаптирован** и не
компилируется на текущих типах.

## Текущее состояние

- `cargo test -p mediaa-frontend --lib` — **334 passed**.
- Старый baseline — **386 passed**.
- Не хватает примерно 52 тестов; 54 старых теста минус возможные замены.
- UI-тесты находятся в `crates/frontend/src/app/ui_tests.rs`,
  `ui/region_editor.rs` и т.п.; отдельного integration-таргета нет.
- `AgentUi`/`live_buffer`/`requests` уже не используют `AgentServices`.
- В frontend нет `mediaa-agent`, `mediaa-engine`, `mediaa-queue`,
  `mediaa-backend`, `mediaa-storage` даже в dev-зависимостях.

## Что запрещено

Нельзя просто скопировать старые тесты в новые файлы. Это вернёт:

- `use mediaa_queue::{FakeClock, MemStore, NewTask, Scheduler, Store}`;
- `use mediaa_agent::pipeline`, `mediaa_agent::assistant::chat::Live`;
- `use crate::agent_host::AgentServices`;
- прямые вызовы `sched/store/feed`.

Это нарушит контракт frontend и `boundaries`. Тесты должны использовать
`AgentSnapshotWire`, `AgentTaskViewWire`, `live_buffer`, `requests` и
`AgentCommand`.

## Соответствие старых и новых типов

| Старое | Новое |
|---|---|
| `mediaa_queue::Task` | `features::agent::view::Task` |
| `mediaa_queue::TaskId` | `i64` |
| `mediaa_queue::Status` | `features::agent::view::Status` |
| `mediaa_queue::Stats` | `features::agent::view::Stats` |
| `mediaa_queue::WaitReason` | `features::agent::view::WaitReason` |
| `mediaa_queue::Role` | не нужен; поле `Task::role: String` |
| `Task` из `MemStore` | `Task::from_wire(&AgentTaskViewWire)` |
| `read(store, filter)` | `queue::read(&AgentSnapshotWire, &Filter)` |
| `AgentServices.sched/store/feed` | `AgentSnapshotWire` + `live_buffer` + `requests` |
| `Live` из `mediaa_agent` | `mediaa_core::assistant::Live` |
| `queue::ANSWER_KIND`/`TITLE_KIND` | `features::agent::view::{ANSWER_KIND,...}` |
| `AgentView.slots()` | `AgentSnapshotWire.slots` |
| `AgentView.services()` | удалено; UI шлёт `Intent::AgentOp/Request` |

## Что нужно добавить в тестовую инфраструктуру

До портирования тестов нужен один разовый слой. Всё — только `cfg(test)`.

### 1. Fixture для wire-задач

Файл: `crates/frontend/src/ui/pages/agent/test_support.rs`
(объявить `#[cfg(test)] pub(crate) mod test_support;` в `agent/mod.rs`).

Минимальный API:

```rust
pub(crate) fn task(
    id: i64,
    kind: &str,
    status: AgentTaskStatusWire,
) -> AgentTaskViewWire;

pub(crate) fn task_with(
    id: i64,
    kind: &str,
    status: AgentTaskStatusWire,
    f: impl FnOnce(&mut AgentTaskViewWire),
) -> AgentTaskViewWire;

pub(crate) fn snapshot(tasks: Vec<AgentTaskViewWire>) -> AgentSnapshotWire;
pub(crate) fn view_task(wire: &AgentTaskViewWire) -> Task;
```

Обязательные поля `AgentTaskViewWire`:

```text
id, parent, kind, title, scope, status, progress, waiting,
created_ms, updated_ms, started_ms, finished_ms, elapsed_ms,
estimate_ms, error, note, stats, units, resource, model, role,
inline, attempts, input, cursor
```

`AgentStatsWire` заполнять нулями; `slots` — `Status::default()`;
`scopes` — из уникальных `scope`; `kind_stats` — пустой или по тесту.

### 2. Управление `live_buffer`

`live_buffer` — глобальное состояние. Добавить в
`ui/pages/agent/live_buffer.rs` только для тестов:

```rust
#[cfg(test)]
pub(crate) fn reset_for_test();

#[cfg(test)]
pub(crate) fn set_for_test(snapshot: AgentSnapshotWire);
```

Тесты, которые трогают `live_buffer` или `i18n`, обязаны брать
`crate::test_support::ui_test_guard()` и `english_locale()`/
`russian_locale()`.

### 3. Доступ к `requests`/`AgentCommand` в тестах

`requests` использует `OnceLock<Sender<AgentRequestWire>>`. Для тестов
добавить `#[cfg(test)]` канал:

```rust
#[cfg(test)]
pub(crate) fn install_test_channel() -> Receiver<AgentRequestWire>;

#[cfg(test)]
pub(crate) fn clear_for_test();
```

Для `chat` — аналогичный test-only receiver у `install_agent_commands`,
либо один общий тестовый канал на весь `App::new_for_test`.

Важно: `OnceLock` устанавливается один раз. Все тесты, которым нужен
канал, должны использовать один общий `ui_test_guard` и не пытаться
переустановить sender.

### 4. Transcript для dialog/chat

Использовать `tempfile::tempdir()` и
`mediaa_core::transcript::Transcript::open(dir, "store")`.
Не открывать реальные каталоги пользователя.

## План по этапам

### Этап 0. Инвентаризация и защита baseline

1. Прочитать `AGENTS.md`, `CLAUDE.md`, `specs/principles.md`,
   `specs/rules.md`, этот файл и `removed-agent-ui-tests/README.md`.
2. Проверить, что текущий baseline — 334:
   ```powershell
   cargo test -p mediaa-frontend --lib
   ```
3. Не менять production-код на этом этапе.
4. Зафиксировать в progress строку `TST-1` как `в работе`, когда начнётся
   реализация.

**Готово:** baseline 334, task/progress прочитаны, исходники найдены.

### Этап 1. Тестовая инфраструктура

1. Добавить `ui/pages/agent/test_support.rs` с builders из раздела выше.
2. Добавить `live_buffer::reset_for_test`/`set_for_test`.
3. Добавить test-only канал `requests` и `chat`, если он нужен
   dialog/chat-тестам.
4. Убедиться, что production не изменился:
   ```powershell
   cargo check --workspace --all-targets
   cargo test -p mediaa-frontend --lib boundaries
   ```

**Готово:** фикстуры компилируются, 334 теста всё ещё зелёные, production
не тянет новых зависимостей.

### Этап 2. Queue — 13 тестов

Файл: `ui/pages/agent/queue.rs`, в конец `#[cfg(test)] mod tests`.

Порядок:

1. `snapshot_of_the_task_details_modal` — Windows-only snapshot; взять
   `Task::from_wire`, вызвать текущий `task_details::show`, snapshot
   `queue-task-details`.
2. `the_load_lists_pools_and_the_web_tool` — `queue::rows(&slots::Status)`,
   без изменений.
3. `rows_flatten_in_order_and_open_on_demand` — заменить `tree()` на
   `wire_tree()`; `read(&snapshot, &Filter::default())`.
4. `filters_pick_the_group_and_the_recording` — snapshot scopes вместо
   `Store`.
5. `the_time_column_says_elapsed_left_or_the_duration` — `time_text`.
6. `a_composite_counts_its_children_and_sums_their_work` — `subtree_stats`.
7. `a_tool_row_shows_the_tool_its_arguments_and_the_size_of_the_result` —
   `row_texts` с `input`/`result` в wire.
8. `the_row_says_the_label_and_the_error` — `error` в wire.
9. `a_note_is_words_in_the_language_of_the_window` — `Note`/`note_text`;
   locale guard.
10. `actions_follow_the_state_and_the_kind_of_task` — `actions`.
11. `the_actions_of_a_composite_reach_its_subtree` — `descendants`/subtree.
12. `the_columns_stay_put_whatever_the_depth` — `geom`.
13. `a_click_opens_details_and_only_the_chevron_folds` — `row_intent`.

Старый `tree()` создаёт `MemStore` и `Scheduler`; заменить на:

```rust
fn wire_tree() -> (AgentSnapshotWire, i64, Vec<i64>) {
    // analysis parent + transcribe/index/topics children
}
```

`FakeClock` не нужен: `now_ms` и `started_ms` задаются явно.

**Готово:** +13 тестов, `queue`-фильтр зелёный, snapshot `queue-task-details`
не изменился или diff просмотрен.

### Этап 3. Analysis — 15 тестов

Файл: `ui/pages/agent/analysis.rs`.

Использовать `mediaa_core::analysis::{Stage, Readiness, readiness}` и
`features::agent::view::{Task, Status, KindStat}`. `mediaa_agent::pipeline`
и `mediaa_queue` не импортировать.

Тесты:

1. `stages_are_offered_as_redo_or_add_by_the_data`
2. `while_an_analysis_runs_only_stop_is_usable`
3. `the_names_item_follows_the_run`
4. `the_menu_reads_the_running_analysis`
5. `a_restart_rebuilds_the_stage_and_every_later_enabled_one`
6. `the_confirmation_lists_what_is_rebuilt`
7. `the_options_keep_their_dependencies`
8. `availability_follows_the_data`
9. `time_is_said_in_minutes`
10. `rows_follow_the_tasks_of_the_analysis`
11. `rows_of_earlier_data_failures_and_counts`
12. `row_texts_for_each_state`
13. `the_row_columns_share_the_header_geometry`
14. `a_running_stage_leads_the_compact_line_and_an_empty_error_has_no_bare_dash`
15. `topics_and_the_breakdown_count_as_running`

Хелпер `bare_ready()` из старого модуля заменить на локальный
`ready_fixture()`.

**Готово:** +15, `analysis`-фильтр зелёный, тесты не используют
`mediaa_agent`/`mediaa_queue`.

### Этап 4. Task details — 13 тестов

Файл: `ui/pages/agent/task_details.rs`.

Использовать `features::agent::view::{Task, Stats, Status}` и текущую
сигнатуру:

```rust
task_details::show(ctx, &task, &stats, now_ms, log, &mut copied, &roster)
```

Тесты:

1. `the_facts_name_the_task_its_input_and_its_models`
2. `a_running_task_counts_its_duration_up_to_now`
3. `the_reason_the_watchdog_interrupted_the_task_is_shown`
4. `a_note_is_shown_instead_of_an_older_error_and_in_the_window_language`
5. `a_cut_topic_detail_says_why_it_stopped`
6. `a_speaker_names_run_lists_the_names_and_the_evidence`
7. `speakers_count_as_the_input_when_there_are_no_options`
8. `known_failures_get_a_hint`
9. `the_model_hint_tells_the_user_to_download`
10. `the_report_carries_the_error_the_hint_and_the_log`
11. `the_log_excerpt_keeps_the_failure_and_its_neighbours`
12. `an_empty_window_shows_the_records_nearest_the_failure`
13. `without_a_time_the_excerpt_is_the_task_or_the_tail`

**Готово:** +13, `task_details`-фильтр зелёный.

### Этап 5. Dialog — 5 тестов

Файл: `ui/pages/agent/dialog.rs`.

Старый `refresh(Some(&services))` больше не существует. Заменить на:

- `requests::start("search", ...)`, получить id;
- `requests::apply(AgentResponseWire { request: id, state: Done{...} })`;
- вызвать текущий `refresh()`/`poll` UI;
- проверить `replies`/`hits`/`names`.

Тесты:

1. `an_automatic_name_says_what_shows_it`
2. `clicks_find_words`
3. `sync_follows_only_what_left_the_view_and_names_save_together`
4. `the_latest_task_of_the_file`
5. `an_event_reloads_the_recording`

Нужны `Transcript` во временном каталоге и `live_buffer` snapshot.
`an_automatic_name` — про `NameSource`; `clicks_find_words` — про
`Hit`/request payload.

**Готово:** +5, dialog-тесты зелёные, `AgentServices` не импортируется.

### Этап 6. Chat — 8 тестов

Файл: `ui/pages/agent/chat.rs`.

Использовать `mediaa_core::assistant::Live`, `features::agent::view`,
`live_buffer` и `AgentCommand`. Не импортировать `mediaa_agent`.

Тесты:

1. `a_queued_cursor_with_steps_continues_only_when_there_is_no_live_stream`
2. `a_chat_without_a_title_is_named_again_once`
3. `citations_become_seek_links`
4. `history_pairs_questions_with_answers`
5. `tabs_close_to_a_neighbour_restore_and_start_a_new_chat`
6. `a_message_sent_while_the_answer_runs_is_a_correction_of_it`
7. `corrections_wait_under_the_answer_until_taken`
8. `a_chat_without_a_title_is_a_new_chat_until_it_is_named`

Тесты 5–8 требуют `Chat`/`Transcript`/`AgentCommand`; тесты 1–4 — timeline
и `Live`. Проверять команды через тестовый receiver, а не через
`Scheduler`.

**Готово:** +8, chat-тесты зелёные, `send`/`stop`/`steer` не вызывают
backend напрямую.

### Этап 7. Backend-owned сценарии

Не все старые тесты должны вернуться во frontend:

- очередь `Scheduler`, `MemStore`, `begin_inline`, ресурсы, тиканье;
- сохранение частичного ответа в `agent_stop`;
- title-задача;
- status/pause/resume.

Их место — `crates/backend/src/daemon/agent.rs` или
`features/agent` backend-тесты. Если сценарий проверяет backend-переход,
переносить его в backend, а во frontend оставить view-проверку.

## Итоговая матрица

| Этап | Тестов | Ожидаемый total |
|---|---:|---:|
| baseline | — | 334 |
| queue | 13 | 347 |
| analysis | 15 | 362 |
| task_details | 13 | 375 |
| dialog | 5 | 380 |
| chat | 8 | 388 |

Цель — не просто 386, а 334 + все применимые старые тесты. Если часть
chat/dialog окажется backend-поведением, она переезжает в backend, а
frontend-число может быть меньше 388 — это допустимо, если покрытие
сценария сохранено.

## Проверки

После каждого этапа:

```powershell
cargo test -p mediaa-frontend --lib <filter>
cargo check --workspace --all-targets
```

Перед завершением:

```powershell
cargo test -p mediaa-frontend --lib
cargo test -p mediaa-backend --lib
cargo test -p mediaa-protocol
cargo test -p mediaa-arch
cargo clippy --workspace --all-targets
cargo fmt --check
```

UI-снапшоты:

- не обновлять автоматически;
- сначала открыть `.diff.png`/`.new.png`, сравнить;
- `UPDATE_SNAPSHOTS=1` применять только после визуальной проверки.

## Риски и подводные камни

1. **Глобальный `live_buffer`.** Тесты могут влиять друг на друга;
   обязателен reset + serial guard.
2. **Глобальная locale.** `english_locale`/`russian_locale` + guard.
3. **`OnceLock` каналов.** Нельзя переустанавливать sender в каждом тесте.
4. **Снапшоты.** `queue-task-details` уже существует; не перезаписывать без
   просмотра.
5. **Параллельные UI-тесты.** Один прогон уже давал transient-падения
   `key_pages_stay...` и `convert_batch_conflict`; финальный прогон был
   зелёным. Не лечить это ослаблением тестов.
6. **Не возвращать `mediaa_queue`/`mediaa_agent`.** Даже как dev-dep.
7. **Не добавлять `#[allow(dead_code)]`** ради тестов.
8. **Не коммитить** без отдельного разрешения.

## Definition of Done

- Все применимые старые тесты портированы; frontend total ≥ 386 или
  задокументировано, какие сценарии ушли в backend.
- `cargo tree -p mediaa-frontend -e all` без backend/storage/agent/engine/queue.
- `cargo test -p mediaa-frontend --lib` — все зелёные.
- `cargo test -p mediaa-backend --lib` — все зелёные.
- `cargo test -p mediaa-arch` — 6 passed.
- `cargo clippy --workspace --all-targets` — без новых warning’ов.
- `cargo fmt --check`.
- `specs/progress.md` строка `TST-1` — `готово`.
- В `removed-agent-ui-tests/` файлы остаются как исторический источник;
  после портирования их можно пометить `archive/`, но не удалять без
  причины.
