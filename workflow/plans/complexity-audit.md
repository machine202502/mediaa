# Аудит избыточной сложности MediaA и план упрощений

## Вывод

Кодовая база дисциплинированная, но две области несут неоправданную сложность: WebP-планировщик
(`crates/webp-pipeline`, `live_webp`) и фоновый lifecycle приложения (`app/tasks`, capture,
профили записи, library removal). Это публичный API без потребителей, несколько представлений
одного решения, дублирующие механизмы остановки и учёта задач. Упрощения ниже уменьшают объём
кода и стоимость изменений LLM-агентом, не меняя UI/UX, форматы результата, каскад
`live WebP → сегментный WebP → линейный FFmpeg → исходный MKV`, отмену, recovery и атомарную
публикацию.

Самый большой эффект даёт чистка `webp-pipeline` (удаление неиспользуемого API и дублирующего
учёта памяти) и централизация фоновых задач. Гипотезы с риском для UX (удаление live WebP,
rescan) вынесены отдельно и требуют измерения.

## Подтверждённая избыточность

Порядок — по ожидаемому эффекту. Каждая находка проверена по коду.

### 1. WebP-планировщик: мёртвый публичный API и настройка без потребителей

- **Решение:** `PipelineTuning` (8 полей), builder-методы `PipelineConfig` (`with_memory_bytes`,
  `with_workers`, `with_attempts`, `with_segment_frames`, `with_retry_backoff`, `with_tuning`,
  `with_trace_id`), а также `RunReport`, `Degradation`, `SegmentPlan.estimated_bytes` и публичный
  `plan`.
- **Текущая польза:** в production задаётся только `max_workers`; всё остальное меняется лишь в
  тестах `webp-pipeline` и `animation_bench`. `SegmentedWebpEncoder` отбрасывает `Ok(report)`,
  `PipelineError.report` никто не читает, `plan` вызывается только внутри крейта.
- **Неоправданные затраты:** восемь настроечных ручек и тип `PipelineTuning` протянуты через
  `AnimationEncodeRequest`, `RecordingSession`, `LiveWebp`; каждый call-site таскает структуру,
  которая фактически равна «defaults + max_workers». `RunReport`/`Degradation` добавляют учёт,
  который нужен только собственным тестам.
- **Предлагаемое упрощение:** оставить в конфиге один вложенный `SchedulePolicy` с реально
  используемыми полями (как минимум `max_workers`, остальные поля — константами внутри крейта);
  удалить builder-методы, существующие только ради тестовых комбинаций; `run` возвращает успех
  или типизированную ошибку, статистику писать в лог; удалить `RunReport`, `Degradation`,
  `estimated_bytes` и публичный `plan`, если после удаления у него нет внешнего потребителя.
- **Обоснование:** поведение не меняется — удаляются только нити данных без production-читателя.
  Явный `max_workers` упрощает поток данных для LLM.
- **Проверка:** `rg` по workspace на отсутствие потребителей; `cargo check --workspace --lib
  --bins --profile dev-check`; `cargo test -p mediaa-webp-pipeline -p mediaa --lib`.

### 2. Четыре представления одного выбора WebP-маршрута

- **Решение:** `segmented_webp` + `parallel_webp` (`RecordSettings`),
  `WebpMode::{Direct,Segmented,SegmentedParallel}` (`recorder/request.rs`),
  `AnimationPath::{Linear,Segmented,Live}` (`recorder.rs`) и
  `WebpSegmentation::{Direct,Segmented}` (`animation_backends.rs`).
- **Текущая польза:** различает уровень записи, уровень кодировщика и внутренний путь.
- **Неоправданные затраты:** ручные преобразования в `animation_path`, `encode_with_linear_fallback`
  и `backend(...)`; четыре синхронизируемых понятия; boolean-пара разрешает невозможную
  комбинацию `segment=false, parallel=true`, которую UI блокирует отдельно.
- **Предлагаемое упрощение:** один persisted enum `WebpPipelineMode::{Direct, Segmented, Live}`,
  передаваемый без преобразований; в encoder request оставить `Direct | Segmented`, если это
  нужно интерфейсу кодировщика; для не-WebP формата нормализовать в `Direct`.
- **Обоснование:** то же поведение и тот же fallback-каскад при меньшем числе сущностей.
- **Проверка:** roundtrip настроек; UI-тесты трёх режимов; pipeline-тесты
  `direct / live / failed-live / segmented-failure`.

### 3. Динамический `MemoryBudget` WebP

- **Решение:** lock-free CAS-резервирование памяти (`memory.rs`), `peak_reserved_bytes`, отказ
  резервирования как ошибка сегмента.
- **Текущая польза:** не дать параллельным воркерам превысить лимит памяти.
- **Неоправданные затраты:** `worker_limit` уже выбирает число воркеров по тому же расчёту
  `memory_bytes / segment_memory`, поэтому в корректном плане отказ резервирования практически
  недостижим; реальный расход FFmpeg/libwebp не измеряется — бюджет опирается на эвристику
  `pixels * quality / 100`, а не на measurement.
- **Предлагаемое упрощение:** удалить `MemoryBudget` и динамические резервы; расчётный пик
  вычислять как `workers * segment_memory`; стресс реального расхода памяти проверять отдельным
  сценарием, а не self-accounting.
- **Обоснование:** бюджет защищает от грубой оценки и потому сам является дублирующей защитой;
  его удаление не ослабляет статический расчёт числа воркеров и размеров сегментов.
- **Проверка:** тесты планирования и budget-pressure; Windows Job Object stress WebP при 256/512/
  768 МБ с контролем peak working set.

### 4. Повтор и деградация при любой ошибке сегмента

- **Решение:** `Schedule::run` при любой ошибке сегмента уменьшает воркеры, дробит сегменты
  (`split_pending`) и повторяет до `max_attempts` с backoff.
- **Текущая польза:** восстановление после transient-сбоя или нехватки памяти.
- **Неоправданные затраты:** для битого входа, неверных параметров и постоянного отказа это не
  исправляет причину, а откладывает внешний линейный fallback; усложняет `Schedule` (дробление
  сегментов в полёте, базовый `completed_us`).
- **Предлагаемое упрощение:** `SegmentBackend` явно сообщает resource exhaustion; только такой
  сбой запускает текущую последовательность деградации; ошибки `normalize`, `validate` и panic
  считать постоянными и сразу возвращать в линейный fallback.
- **Обоснование:** сохраняет отказоустойчивость там, где она реальна (OOM), и ускоряет
  диагностику/fallback в остальных случаях.
- **Проверка:** тест постоянной ошибки (одна попытка, немедленный fallback) и тест resource
  exhaustion (полная деградация, успех). Риск распознавания OOM — см. «Риски».

### 5. `join_with_timeout` и join-watchdog у capture

- **Решение:** `join_with_timeout` запускает отдельный поток, который блокируется в `handle.join()`,
  чтобы финализация не зависла на остановке native capture.
- **Текущая польза:** ограниченная остановка capture.
- **Неоправданные затраты:** при действительно зависшем capture и рабочий поток, и watchdog
  остаются заблокированными (утечка потока на каждый таймаут); дублирующий `reap_capture`.
- **Предлагаемое упрощение:** capture-поток после `ready` отправляет терминальное событие
  (результат или ошибка); `RecordingSession` ждёт его с `capture_stop_timeout`; при timeout поток
  отсоединяется без второго join-потока, рабочий каталог сохраняется для recovery. Удалить
  `TimedJoin`, `join_with_timeout`, `reap_capture`.
- **Обоснование:** тот же контракт остановки без лишнего потока; recovery уже умеет подбирать
  рабочие каталоги.
- **Проверка:** тесты capture `ready / success / error / panic / timeout / detach` без второго
  потока; канал результата остаётся ограниченным и вычитывается при остановке.

### 6. Дублирующий источник FPS и высоты у профилей записи

- **Решение:** `RecordSettings.fps`/`output_height` и `AnimSettings.fps`/`max_height` хранят одни
  и те же значения; `recording_animation_settings` перезаписывает `AnimSettings` из
  `RecordSettings` при старте.
- **Текущая польза:** отдельный тип анимации.
- **Неоправданные затраты:** два источника истины для одного значения; риск рассинхронизации.
- **Предлагаемое упрощение:** persisted-настройки анимации без FPS/высоты (палитра GIF, dithering,
  WebP quality); владельцем FPS/высоты записи оставить `RecordSettings`; полный `AnimSettings`
  собирать перед `RecordingRequest`; настройки конвертации сохраняют свои FPS/высоту отдельно.
- **Обоснование:** убирает перезапись и лишнее поле без изменения UI.
- **Проверка:** roundtrip настроек; точное формирование `RecordingRequest` для WebP/GIF/видео.

### 7. Три модели фоновых задач и quirk clipboard→Saving

- **Решение:** `TaskKind` (19 вариантов), ручной disposable-список в `App::quit`,
  битсет `ActiveTasks`/`WorkKind`; `update_tray` показывает `Saving` при занятом clipboard.
- **Текущая польза:** различие обязательных и отбрасываемых операций для выхода.
- **Неоправданные затраты:** новый `TaskKind` легко забыть в ручном списке; занятый clipboard
  переводит tray в `Saving`, хотя сохранения нет; три пересекающихся модели.
- **Предлагаемое упрощение:** при регистрации хранить `TaskPolicy::{Required, Disposable}`;
  заменить ручной список на `detach_disposable()`; quit/tray состояние вычислять из policy;
  удалить `ActiveTasks`, когда recording/conversion/recovery/cleanup/image jobs будут в
  `TaskSupervisor` или явном `ServiceBusy`.
- **Обоснование:** одна таблица `TaskKind → policy` вместо трёх мест, которые нужно держать в
  синхроне; исправляет ложный индикатор tray.
- **Проверка:** exhaustive-тест policy для каждого `TaskKind`; quit-сценарии с каждой
  disposable/required категорией; clipboard не показывает `Saving`.

### 8. Test-only публичный API

- **Решение:** `PreviewLimits`, `ensure_proxy_for_with_limits` и `Settings::save()`.
- **Текущая польза:** только тесты; production использует `ensure_proxy_for` и `SettingsWriter::save_to`.
- **Неоправданные затраты:** публичный контракт без потребителя; лишняя поверхность для изменений.
- **Предлагаемое упрощение:** оставить preview 1080p/30 fps локальными константами; удалить
  `Settings::save()` и `ensure_proxy_for_with_limits`; без compatibility aliases.
- **Обоснование:** нет production-потребителя, значит нет риска удаления.
- **Проверка:** `rg` по workspace; тесты preview graph и roundtrip settings.

### 9. Дублирование library removal jobs

- **Решение:** `start_delete` и `start_cleanup` почти полностью дублируют создание job (занятость,
  preview/viewer, spawn, channel, task id, polling); одиночное удаление создаёт фиктивный
  `CleanupPlan { older: Week }`; успех хранится отдельно в `cleanup_success`.
- **Текущая польза:** два сценария удаления.
- **Неоправданные затраты:** две почти одинаковые трассы выполнения и скрытое состояние.
- **Предлагаемое упрощение:** один `RemovalJob { plan, mode, success }` и один запуск для `Aged`
  и `Selected`; результат `RemovalOutcome { plan, report, success }` через тот же канал; убрать
  `cleanup_success`; сохранить разные правила проверки возраста.
- **Обоснование:** меньше кода при тех же UI-правилах.
- **Проверка:** существующие UI-тесты delete/cleanup, partial-failure, stale-age, trash/proxy.

## Гипотезы, требующие измерения

- **Live WebP (`SegmentedParallel`).** Это крупнейшая отдельная подсистема: `live_webp.rs`
  (~450 строк), менеджер с парсингом flat segment-list от FFmpeg, worker-пул, per-chunk
  normalize/validate/merge, `AnimationPath::Live` в finalize. Её польза — почти мгновенный merge
  после остановки вместо пост-кодирования. Гипотеза: если задержка после остановки у офлайн-
  сегментного пути приемлема, можно удалить live-путь целиком и сделать `Segmented` (ограниченная
  память, корректный результат) дефолтом WebP-записи. Гейт — измерить
  «стоп → файл виден» для длинной записи на обоих путях. Без замера не удалять.
- **Безусловный rescan каждые 60 с.** Из `library-performance-research.md`: локально лишний,
  на 50k даёт frame hitch; watcher/overflow уже покрывают потерю событий. Упрощение допускается
  только после network-share замера (rescan по watcher failure/overflow, ручному Refresh и явной
  стратегии для сетевых путей).
- **Thumbnail atlas.** Замена отдельными JPEG отклонена измерениями: в ~8,5 раз больше диска и
  ~188 раз дольше запись. Atlas оставить.

## Дополнительные дефекты, найденные при чтении кода (не сложность)

- **`crates/platform/src/single_instance.rs`, unix-ветка не компилируется.** `imp::activate_existing()`
  вызывает `super::development_scope()`, которой в модуле нет (в `mediaa_core::dev` есть только
  `instance_scope`). Linux/macOS-цель почти наверняка не собирается, несмотря на README.
  Исправление — заменить на `mediaa_core::dev::instance_scope()`. Подтвердить Linux-сборкой.
- **Остаточный мусор.** `crates/agent/target/live-names-ai/*.db` (чужой удалённый проект), пустое
  дерево `crates/app/crates/app/tests/snapshots/` (настоящие снапшоты — в `crates/app/tests/snapshots`),
  gitignored `evals/local/hh-experience`. Удаление локальных файлов — только после подтверждения
  пользователя; трекаемые в git файлы не удаляются без отдельного разрешения.

## Отражение предложений из документов

Все десять предложений `runtime-complexity-simplification.md` приняты и перенесены в этапы ниже.
Выводы `library-performance-research.md` приняты как гипотезы/решения выше (rescan — отложить,
atlas — оставить). Полная оценка — в [complexity-audit-review.md](complexity-audit-review.md).

## План исполнения

Этапы выполняются строго по одному; каждый оставляет проект в рабочем состоянии. Работу вести по
`CLAUDE.md`: задачи в `workflow/tasks.md` по одной, статусы `ожидание → в работе → готово/отказ`.

### Этап 0. Входной gate

Убедиться, что UI/UX-контур зелёный (`ui-ux-regression-tests.md`): mocked devices, screenshot,
video/audio, settings, library, conversion, responsive, WebP failure logging, single-instance.
Snapshot-тесты — на Windows.

### Этап 1. Удаление мёртвого API (без изменения поведения)

Файлы: `crates/core/src/settings.rs` (`Settings::save`), `crates/media/src/preview_cache.rs`
(`PreviewLimits`, `ensure_proxy_for_with_limits`), `crates/webp-pipeline/src/lib.rs` и
`scheduler.rs` (`RunReport`, `Degradation`, `estimated_bytes`, публичный `plan`, лишние
builder-методы), `crates/media/src/animation_backends.rs` (упростить использование `run`).

Действия: удалить перечисленное; статистику `run` писать в лог; `SchedulePolicy` вместо
`PipelineTuning` (или минимум — оставить только `max_workers`).

Проверка: `rg` на отсутствие потребителей; `cargo check --workspace --lib --bins --profile
dev-check`; `cargo test -p mediaa-webp-pipeline -p mediaa --lib`; `cargo clippy --workspace
--all-targets`.

### Этап 2. WebP-планировщик: память и политика повторов

Файлы: `crates/webp-pipeline/src/{memory,scheduler,lib}.rs`.

Действия: удалить `MemoryBudget`; расчётный пик считать арифметически; ввести явный сигнал
resource exhaustion в `SegmentBackend`; постоянные ошибки (`normalize`, `validate`, panic) сразу в
линейный fallback; деградация только для resource exhaustion.

Проверка: тесты planning, budget-pressure, постоянной ошибки, отмены, merge; Windows Job Object
stress WebP 256/512/768 МБ.

### Этап 3. Один enum WebP-маршрута

Файлы: `crates/core/src/settings.rs`, `crates/media/src/recorder/{request,recorder}.rs`,
`crates/media/src/animation_backends.rs`, `crates/app/src/pages/record.rs`,
`crates/app/src/app/recording.rs`.

Действия: `WebpPipelineMode::{Direct, Segmented, Live}` persisted; убрать `WebpMode`,
`AnimationPath`, `WebpSegmentation` и ручные преобразования; сохранить fallback-каскад.

Проверка: roundtrip settings, UI-тесты трёх режимов, pipeline-тесты direct/live/failed-live/
segmented-failure.

### Этап 4. Один источник FPS/высоты профилей

Файлы: `crates/core/src/settings.rs`, `crates/app/src/app.rs`, `crates/app/src/pages/{record,
convert,settings}.rs`.

Действия: persisted-анимация без FPS/высоты; сборка полного `AnimSettings` перед
`RecordingRequest`; конвертация сохраняет свои значения.

Проверка: roundtrip и профильные тесты, UI-тесты record/convert/settings.

### Этап 5. Capture через терминальное событие

Файлы: `crates/media/src/recorder.rs`, `crates/media/src/recorder/{capture,tests}.rs`.

Действия: capture-поток шлёт терминальное событие; ожидание с `capture_stop_timeout`; удалить
`TimedJoin`, `join_with_timeout`, `reap_capture`.

Проверка: тесты capture ready/success/error/panic/timeout/detach без второго потока.

### Этап 6. `BackgroundJob<T>` и `TaskPolicy`

Файлы: `crates/app/src/app/{tasks,state,actions}.rs`, контроллеры one-shot операций.

Действия: сначала перевести `LibraryLoad`, `LibraryScan`, `ConversionProbe`; затем остальные;
`TaskPolicy::{Required, Disposable}` при регистрации; удалить ручной список и `ActiveTasks`;
исправить clipboard→Saving.

Проверка: exhaustive policy-тест, quit/tray-сценарии, тесты `BackgroundJob<T>` (result, panic,
cancel, deadline, drop).

### Этап 7. Унификация library removal

Файлы: `crates/app/src/pages/library.rs`, `crates/app/src/pages/library/{actions,cleanup}.rs`.

Действия: один `RemovalJob` и `RemovalOutcome`; убрать `cleanup_success` и фиктивный `CleanupPlan`.

Проверка: UI-тесты delete/cleanup, partial-failure, stale-age, trash/proxy.

### Этап 8. Документация, очистка, non-Windows фикс

Действия: обновить `README.md`, `workflow/stones.md`, `workflow/practices.md` (только ставшее
устойчивым знание); исправить `single_instance.rs` и подтвердить Linux-сборкой; очистить
остаточный мусор (с подтверждением пользователя).

## Итоговая проверка

- `cargo test --workspace`;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `rg` на отсутствие удалённых символов;
- UI snapshot-тесты на Windows;
- ручная запись WebP: обычный план, memory-pressure, неподвижный сегмент;
- сохранение исходного MKV при окончательном отказе WebP.

## Риски

- Неизвестная ошибка FFmpeg, ранее лечившаяся дроблением, уйдёт в линейный fallback: результат
  сохраняется, меняется время завершения и журнал диагностики.
- Распознавание resource exhaustion по ошибке FFmpeg может быть неточным; если надёжной
  классификации нет — не вводить неверную policy (см. review).
- Миграция one-shot задач может потерять panic/deadline; выполнять малыми группами.
- Capture-канал должен оставаться ограниченным и вычитываться при остановке.
- Ошибочная `TaskPolicy` может не закрыть приложение или потерять обязательную операцию;
  покрыть exhaustive-тестом.
- Замена двух boolean на enum меняет сериализованные настройки; выпусков нет, миграция не нужна.
- Удаление live WebP и rescan — только после замеров; в рамках этого плана они остаются.

## Вне рамок

- изменение периодического rescan до network-share замера;
- переработка thumbnail atlas;
- изменение таймаутов audio, native capture и сохранения настроек;
- изменение форматов результата, fallback-каскада и пользовательской компоновки;
- release-сборка без отдельного запроса.
