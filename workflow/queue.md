# Очередь и исполнители — проект (todos 14.10, включает 14.6–14.8, 14.12, 14.14 в части очереди)

Контракт для исполнителей. Расхождения сначала правятся здесь. Почему: todos 14.10 (сценарии
пользователя 2026-10-22 — скрытые прогоны после паузы, ресурс не освобождается, один исполнитель,
неверные статусы родителей, студия не обновляется).

## 1. Крейт `mediaa-queue`
Без зависимостей от движка, агента, UI. Модули:
* `state` — чистые функции: статусы `queued | running | paused | waiting(composite) | done |
  failed | cancelled`, таблица переходов (ниже), пересчёт родителей (правило 13.7), зависимости.
* `store` — трейт хранилища; `SqliteStore` (как сейчас, схема без миграции — пересоздать), `MemStore`
  для тестов. Каждая операция — одна транзакция.
* `scheduler` — пулы исполнителей по видам ресурса (`speech`, `embed`, `llm`, `provider`; ёмкость
  и память из бюджета/слотов), справедливость, раздача, остановка, пульс, продолжение после старта.
* `clock`, `runner` — трейты: время; «как выполнить задачу вида X» (реализует агент).
* `events` — канал событий `{task, scope, kind: started|chunk|paused|resumed|done|failed|cancelled}`.
* `snapshot` — то, что видит UI: задачи с прогрессом и временем, «что идёт сейчас» (задача, кусок,
  сколько), у ждущих — причина (`dependency`, `resource(kind)`, `memory`, `paused`, `turn`).

## 2. Переходы (кто → что)
| из | пользователь | исполнитель | система |
|---|---|---|---|
| queued | пауза → paused; отмена → cancelled (+зависимые) | взять → running | — |
| running | пауза → **stopping**→paused; отмена → stopping→cancelled | кусок → running; готово → done; ошибка → failed (+зависимые cancelled «из-за ошибки в …») | падение программы → queued (с последнего куска) |
| paused | продолжить → queued (только после фактической остановки) ; отмена → cancelled | — | — |
| failed/cancelled | заново → queued (+отменённые из-за неё) | — | — |
`stopping` — внутреннее: флаг остановки поднят, исполнитель обязан вернуть управление на следующем
токене/куске; задача не берётся снова, пока прогон не завершён (нет скрытых прогонов). Ресурс
освобождается в момент возврата, и сразу раздаётся следующему.

## 3. Работа кусками (14.6)
`runner` получает `ctx` с `checkpoint(cursor, partial)`, `should_stop()`, `progress()`. Правило: не
дольше одного куска без `checkpoint`; после падения — продолжение с курсора. Куски: распознавание —
фраза; диаризация — отрезок 5–10 мин (склейка голосов в конце, 14.4); индекс — пачка 32; сводка/
разбор темы — окно; склейка — решение; ответ чата — шаг агента (состояние цикла в курсоре, 14.6).

## 4. Справедливость
Участник = ответ чата или анализ записи. Раздача по кругу по кускам; чаты приоритетнее, фон получает
не меньше каждого N-го куска (N=3); внутри группы — строгий круг. Ожидающий чат: причина `turn`.

## 5. Связь с движком (14.14)
Очередь сообщает движку «какие модели нужны ждущим задачам» → эти модели выгружаются последними.

## 6. Тесты (обязательны)
`proptest`-симуляция на `MemStore` + подставные время и исполнители (работают, падают, зависают,
долго не останавливаются): после каждого шага — нет скрытых прогонов; ресурс свободен после паузы;
никто не ждёт отменённого/упавшего; статусы родителей по 13.7; готовое не делается дважды; время
верно; после «падения» — продолжение с места; справедливость (никто не ждёт дольше K кусков);
очередь доходит до конца. Сценарии пользователя дословно (todos 14.10).

## 7. Переход
Старое `mediaa_core::queue` и `agent::worker` удаляются; агент реализует `runner` для своих видов;
`pipeline` только ставит задачи; UI читает `snapshot` и подписан на `events` (студия перечитывает
готовность и данные записи по событию своей записи).

## 8. Сделано (2026-10-22) и решения при подключении
* Старые `mediaa_core::queue` и `agent::worker` удалены; база — та же `.ai/queue.db`, схема
  создаётся заново. Исполнитель — `step()` опрашивает кусок, идущий в своём потоке; `should_stop`
  связан с отменой вызова модели.
* Ответ чата — обычная задача (`chat-answer`, пул `llm`/`provider`, ключ справедливости — чат),
  кусок — шаг агента, курсор — состояние цикла; шаги и инструменты внутри — inline-строки.
* «Заново» у упавшей задачи начинает с нуля (курсор сбрасывается); «Продолжить» — только для паузы.
* Не останавливаются внутри вызова: диаризация (до 14.4/14.6 — одним куском, результат
  отбрасывается), план тем (без модели, быстрый).
* Лист без `resource` — ошибка при постановке.

## 9. Пограничные случаи — каталог (решение пользователя 2026-10-22)
Конечный список; у каждого случая — свой тест с номером случая в имени (`a02_…`). Во всех тестах
после каждого шага проверяется E1. Столбец «Тест» заполняет исполнитель.

| № | Случай | Ожидание | Тест |
|---|---|---|---|
| A01 | Пауза задачи в очереди (не начата) | не берётся; «Продолжить» — берётся | `crates/queue/tests/edge_cases.rs::a01_a_paused_queued_task_is_not_taken_until_resumed` |
| A02 | Пауза посреди куска | остановка за доли секунды, слот свободен; «Продолжить» — с курсора, готовые куски не повторяются | `crates/queue/tests/queue.rs::pause_mid_call_frees_the_slot_and_does_not_repeat_a_chunk` |
| A03 | «Продолжить», пока прогон ещё останавливается | отклонено, второго прогона нет; после остановки — можно | `crates/queue/tests/queue.rs::pause_mid_call_frees_the_slot_and_does_not_repeat_a_chunk` |
| A04 | Отмена задачи на паузе | отменена, зависимые отменены | `crates/queue/tests/edge_cases.rs::a04_cancelling_a_paused_task_cancels_its_dependents` |
| A05 | Двойная пауза / двойное «Продолжить» / пауза готовой | без эффекта, без ошибок состояния | `crates/queue/tests/edge_cases.rs::a05_double_pause_double_resume_and_pausing_a_done_task_do_nothing` |
| A06 | Пауза родителя «Анализ» и продолжение | все листья на паузе; потом идут в порядке зависимостей | `crates/queue/tests/edge_cases.rs::a06_pausing_the_analysis_pauses_every_leaf_and_resume_keeps_the_dependency_order` |
| A07 | Пауза одной стадии | зависимые ждут с причиной «на паузе», независимые идут | `crates/queue/tests/edge_cases.rs::a07_pausing_one_stage_holds_dependents_and_leaves_independent_work_running` |
| A08 | Отмена идущей | отменена, слот свободен, зависимые отменены (пометка «из-за ошибки» — только при ошибке, не при отмене пользователем) | `crates/queue/tests/edge_cases.rs::a08_cancelling_a_running_task_frees_the_slot_and_cancels_dependents` |
| A09 | Ошибка задачи и «Заново» | зависимые отменены «из-за ошибки»; «Заново» поднимает её и отменённые из-за неё, готовое не переделывается | `crates/queue/tests/edge_cases.rs::a09_failure_cancels_dependents_and_restart_brings_back_only_that_work` |
| A10 | «Заново» на готовой стадии | очередь: перезапускается только она; пересборку зависимых с данными делает конвейер («Заново» стадии в студии) | `crates/queue/tests/edge_cases.rs::a10_restart_of_a_done_stage_reruns_it_and_leaves_the_rest_done`; `crates/agent/src/pipeline.rs::restart_from_the_index_cancels_the_analysis_and_keeps_the_words`, `a_restart_from_the_details_rebuilds_them_and_keeps_a_chat_answer` |
| A11 | Падение приложения посреди куска | после запуска — с последнего куска | `crates/queue/tests/queue.rs::a_crash_continues_from_the_last_chunk` |
| A12 | Падение во время паузы | после запуска остаётся на паузе | `crates/queue/tests/edge_cases.rs::a12_a_crash_while_paused_leaves_the_task_paused` |
| A13 | Падение во время «останавливается» | после запуска — на паузе / отменена (по намерению) | `crates/queue/tests/edge_cases.rs::a13_a_crash_while_stopping_settles_each_task_by_its_intent` |
| A14 | Зависание (нет пульса) | в очередь с курсора; поздняя запись старого прогона не принимается | `crates/queue/tests/queue.rs::a_hang_is_reaped_back_to_its_cursor_and_a_late_write_is_ignored` |
| A15 | Последний кусок | готово; родители по 13.7 | `crates/queue/tests/queue.rs::studio_shows_one_recording_ready_when_its_analysis_is_done` |
| B01 | Две записи, разные пулы | идут одновременно | `crates/queue/tests/queue.rs::two_recordings_run_on_different_pools` |
| B02 | Две записи, один пул | по кругу кусками | `crates/queue/tests/edge_cases.rs::b02_two_recordings_on_one_pool_take_turns_by_chunk` |
| B03 | Пауза у A | освободившийся слот в тот же тик берёт B | `crates/queue/tests/queue.rs::pause_mid_call_frees_the_slot_and_does_not_repeat_a_chunk` |
| B04 | Отмена анализа A | B не затронута | `crates/queue/tests/edge_cases.rs::b04_cancelling_analysis_a_leaves_analysis_b_alone` |
| B05 | Ошибка в A | B идёт дальше | `crates/queue/tests/edge_cases.rs::b05_an_error_in_a_does_not_stop_b` |
| B06 | «Продолжить» A, пока B посреди куска | A ждёт своей очереди, B не прерывается | `crates/queue/tests/edge_cases.rs::b06_resuming_a_while_b_is_mid_chunk_waits_for_b_and_does_not_interrupt_it` |
| B07 | A не помещается в память вместе с B | ждёт с причиной «память», берётся после освобождения | `crates/queue/tests/edge_cases.rs::b07_a_task_that_does_not_fit_in_memory_waits_and_is_taken_on_release` |
| C01 | Три записи на одном пуле | строгий круг, никто не ждёт дольше K кусков | `crates/queue/tests/edge_cases.rs::c01_three_recordings_on_one_pool_keep_a_strict_circle` |
| C02 | Пауза средней из трёх и продолжение | две делят ресурс; потом снова круг из трёх | `crates/queue/tests/edge_cases.rs::c02_pausing_the_middle_of_three_lets_two_share_and_the_circle_returns_on_resume` |
| C03 | Отмена всех трёх по очереди | ничего не идёт, слоты свободны, ничего не ждёт | `crates/queue/tests/edge_cases.rs::c03_cancelling_all_three_in_turn_leaves_nothing_running_or_waiting` |
| C04 | Падение приложения с тремя в работе | все продолжают со своих мест | `crates/queue/tests/edge_cases.rs::c04_a_crash_with_three_in_work_continues_each_from_its_place` |
| D01 | Вопрос в чате во время анализа | чат первым, фон не реже каждого 3-го куска | `crates/queue/tests/edge_cases.rs::d01_a_chat_question_during_an_analysis_goes_first_and_background_gets_every_third_chunk` |
| D02 | Два чата на одной модели | вперемешку по шагам; стоп в одном не трогает другой | `crates/queue/tests/edge_cases.rs::d02_two_chats_interleave_and_stopping_one_leaves_the_other_running` |
| D03 | Стоп ответа посреди шага | остановка быстро, слот уходит фону | `crates/queue/tests/edge_cases.rs::d03_stopping_an_answer_mid_step_frees_the_slot_for_the_background` |
| D04 | Падение посреди ответа | продолжение со следующего шага | `crates/queue/tests/edge_cases.rs::d04_a_crash_mid_answer_continues_from_the_next_step` |
| D05 | Ответ провайдером и локальный разбор | параллельно (разные пулы) | `crates/queue/tests/edge_cases.rs::d05_a_provider_answer_and_a_local_analysis_run_in_parallel` |
| E01 | После каждого шага любого случая | снимок = данные (статусы, «что идёт», причины ожидания); событие на каждое изменение | `crates/queue/tests/edge_cases.rs::Probe::check` (вызывается после каждого шага в каждом тесте файла) |
| E02 | После любого случая «продолжить всё» | очередь доходит до конца: всё готово, отменено или упало, никто не ждёт вечно | `crates/queue/tests/edge_cases.rs::drain_to_end` (вызывается в конце каждого теста файла) |
