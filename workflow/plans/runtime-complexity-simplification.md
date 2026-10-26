# Упрощение WebP-планировщика и фонового lifecycle

## Цель

Убрать дублирующие механизмы в планировщике WebP, фоновых операциях приложения и
жизненном цикле записи. Сохранить проверенное поведение:

- один пользовательский артефакт `mediaa`;
- отмену, deadline и обнаружение panic у фоновых операций;
- ограниченную остановку native capture;
- каскад `live WebP → сегментный WebP → линейный FFmpeg → исходный MKV`;
- атомарную публикацию результатов и восстановление незавершённых записей.

Аудит не меняет подтверждённые платформенные защиты. Аудио, FFmpeg jobs, native capture и
удаление временных файлов сохраняют текущие таймауты и порядок очистки.

## Входной gate

Упрощение runtime начинается только после зелёного UI/UX-контура из
[ui-ux-regression-tests.md](ui-ux-regression-tests.md). Gate включает mocked devices, screenshot,
video/audio, settings, library, conversion, responsive layouts, WebP failure logging и
single-instance scopes.

## Принятые решения

### 1. Удалить динамический `MemoryBudget` WebP

`worker_limit` уже выбирает число воркеров по тому же расчёту
`memory_bytes / segment_memory`, который затем повторяет `MemoryBudget::reserve`. В корректном
плане сегментов отказ резервирования фактически недостижим, а расход FFmpeg/libwebp он всё
равно не измеряет.

Решение:

- оставить расчёт числа воркеров, размеров сегментов и шагов деградации;
- удалить `MemoryBudget`, динамические резервы, CAS-учёт и `peak_reserved_bytes`;
- расчётный пик при необходимости вычислять из `workers * segment_memory`;
- сохранить stress-проверку реального расхода памяти отдельным сценарием.

### 2. Ввести типизированную one-shot задачу

Сейчас владелец операции хранит и `TaskId`, и отдельный канал результата. Успешный результат,
disconnect, panic, deadline и detach обрабатываются в нескольких местах.

Решение:

- оставить единый registry активных задач и его контракты отмены/deadline;
- добавить `BackgroundJob<T>`, который объединяет идентичность задачи и типизированный
  результат;
- предоставить общий poll, возвращающий `Pending`, `Finished(T)`, `Cancelled`, `TimedOut` или
  `Panicked`;
- хранить отдельные каналы только у потоковых операций, например каталога устройств и окон.

Сначала на новый контракт переводятся `LibraryLoad`, `LibraryScan` и `ConversionProbe`. После
проверки переводятся остальные one-shot операции приложения.

### 3. Деградировать WebP только при нехватке ресурсов

Сейчас любая ошибка сегмента уменьшает параллельность и длину кадров, дробит сегменты и
повторяется до шести раз. Для битого входа, неверных параметров и постоянного отказа это не
исправляет причину, а задерживает внешний fallback.

Решение:

- `SegmentBackend` явно сообщает, является ли ошибка resource exhaustion;
- только resource exhaustion запускает имеющуюся последовательность уменьшения воркеров,
  дробления сегментов и повторов;
- ошибки `normalize`, `validate` и panic считаются постоянными;
- постоянная ошибка сразу возвращается в каскад линейного fallback;
- число попыток для подтверждённой нехватки памяти остаётся достаточным для текущей
  последовательности деградации.

### 4. Получать результат capture через терминальное событие

`join_with_timeout` запускает дополнительный `join-watchdog`, который при действительно
зависшем native capture остаётся заблокированным вместе с уже зависшим рабочим потоком.

Решение:

- capture-поток после `ready` отправляет терминальное событие с результатом или ошибкой;
- `RecordingSession` ожидает это событие с общим `capture_stop_timeout`;
- `JoinHandle` больше не используется для получения результата;
- при timeout поток отсоединяется без дополнительного join-потока, рабочий каталог сохраняется
  для recovery;
- `TimedJoin`, `join_with_timeout` и дублирующий `reap_capture` удаляются.

### 5. Убрать второй источник FPS и высоты у профилей записи

Для профилей записи `RecordSettings` и `AnimSettings` хранят FPS и высоту, хотя UI редактирует
только `RecordSettings`, а при старте значения `AnimSettings` перезаписываются.

Решение:

- добавить persisted-настройки анимации без FPS и высоты: палитра GIF, dithering и WebP quality;
- единственным владельцем FPS и высоты записи оставить `RecordSettings`;
- полный `AnimSettings` собирать из `RecordSettings` и persisted-настроек анимации перед
  созданием `RecordingRequest`;
- отдельные настройки конвертации сохраняют собственные FPS и высоту;
- старые поля не мигрировать и не создавать compatibility shims.

## Дополнительные подтверждённые кандидаты

Эти решения не входят в уже согласованную первую пятёрку. Они рекомендуются после отдельного
подтверждения, потому что также уменьшают реальную стоимость изменений без удаления необходимой
защиты.

### 6. Удалить отчётность и дублирующий API WebP-планировщика

`RunReport`, `Degradation` и `SegmentPlan.estimated_bytes` не используются production-кодом.
`SegmentWebpEncoder` отбрасывает успешный `RunReport`, а `PipelineError.report` никто не читает.
Подробные данные проверяются только собственными тестами планировщика. Параллельно
`PipelineTuning` и `PipelineConfig` повторяют семь полей, а несколько builder-методов имеют
потребителей только в тестах и benchmark.

Решение:

- вернуть из `run` только успех или типизированную ошибку;
- оставить в ошибке источник и признак отмены, а статистику попыток и деградаций писать в лог;
- удалить `RunReport`, `Degradation`, `estimated_bytes` и публичный `plan`, если после удаления
  у него не останется внешнего потребителя;
- хранить scheduling-параметры одним вложенным `SchedulePolicy` в `PipelineConfig`, вместо
  копирования полей туда и обратно;
- удалить builder-методы, существующие только ради тестовых комбинаций.

Проверка: тесты планирования, budget-pressure, постоянной ошибки, отмены, merge и публичного API
должны проверять результат и побочные файлы, а не внутреннюю статистику.

### 7. Оставить один тип WebP-маршрута

Один и тот же выбор сейчас представлен четырьмя способами:

- `segmented_webp` и `parallel_webp` в `RecordSettings`;
- `WebpMode::{Direct, Segmented, SegmentedParallel}` в `RecordingRequest`;
- `AnimationPath::{Linear, Segmented, Live}` в recorder;
- `WebpSegmentation::{Direct, Segmented}` в encoder request.

Между ними существуют ручные преобразования. Сохранённые boolean-поля допускают невозможную
комбинацию `segment=false, parallel=true`, хотя UI блокирует параллельный переключатель.

Решение:

- использовать один persisted enum `WebpPipelineMode::{Direct, Segmented, Live}`;
- передавать его без преобразования через request и recorder;
- оставить в animation backend только `Direct | Segmented`, если это требуется интерфейсу
  кодировщика;
- при формате, отличном от WebP, нормализовать режим в `Direct` в builder настроек.

Проверка: UI-тесты трёх режимов, roundtrip настроек, direct/live/failed-live/segmented-failure
pipeline tests и сохранение текущего fallback.

### 8. Централизовать policy фоновых задач

Сейчас quit-поведение зависит от трёх независимых моделей:

- 19 вариантов `TaskKind`;
- ручного списка `Disposable` variants в `App::quit`;
- `ActiveTasks`/`WorkKind` bitset для quit и tray.

Новый `TaskKind` нужно не забыть добавить в ручной detach-list, иначе он неверно удержит выход.
`WorkKind::Clipboard` также переводит tray в `Saving`, хотя файл не сохраняется.

Решение:

- при регистрации задачи хранить `TaskPolicy::{Required, Disposable}`;
- заменить ручной список в `App::quit` на `detach_disposable()`;
- вычислять quit/tray processing state из policy и состояния non-task сервисов;
- удалить `ActiveTasks`, когда recording/conversion/recovery/cleanup/image jobs будут либо в
  `TaskSupervisor`, либо представлены явным `ServiceBusy`.

Проверка: таблица `TaskKind → policy`, тесты quit с каждой disposable/required категорией,
clipboard не показывает tray `Saving`, recording/conversion не теряются при выходе.

### 9. Удалить публичные API без production-потребителя

`ensure_proxy_for_with_limits` и `PreviewLimits` используются только тестами, хотя пользователь
не настраивает preview resolution или FPS. `Settings::save()` также не вызывается в проекте:
production сохраняет настройки через `SettingsWriter::save_to`.

Решение:

- оставить preview 1080p/30 fps локальными константами и удалить test-only публичный API;
- удалить `Settings::save()`, оставив `load_from`/`save_to` и production writer;
- не добавлять compatibility aliases.

Проверка: `rg` по workspace, тесты preview graph и roundtrip settings, полный Clippy.

### 10. Унифицировать фоновые removal jobs библиотеки

Одиночное удаление и очистка по возрасту почти полностью дублируют создание job:
проверку занятости, подготовку preview/viewer, spawn, channel, task id и terminal polling.
Одиночное удаление дополнительно создаёт фиктивный `CleanupPlan { older: Week }`, а сообщение
об успехе хранится отдельно в `cleanup_success`.

Решение:

- ввести один `RemovalJob { plan, mode, success }`;
- одним методом запускать `Aged` или `Selected` removal;
- возвращать `RemovalOutcome { plan, report, success }` через тот же channel;
- удалить `cleanup_success` как отдельное скрытое состояние;
- сохранить разные правила проверки: повторная проверка возраста для bulk cleanup, точечная
  обработка для одиночного удаления.

Проверка: существующие partial-failure, stale-age, trash/proxy, modal и UI delete tests.

## Гипотезы, требующие измерения

Подробные измерения записаны в
[library-performance-research.md](library-performance-research.md).

- **Полный rescan каждые 60 секунд.** Локальный тест 1k/10k не нашёл потерянных watcher paths;
  overflow сам запрашивает rescan. На 10k применение полного snapshot занимает 5–11 ms, на
  50k — 29–69 ms. Локально безусловный интервал не дал практической пользы и может пропускать
  кадр на очень большой библиотеке. Решение отложено до network-share проверки.
- **Собственный thumbnail atlas.** Гипотеза отклонена. На 500 thumbnail-файлов atlas занял
  21 файл / 8,7 MB и 1,8 s записи; отдельные JPEG — 500 файлов / 73,6 MB и 332 s. Random access
  у отдельных файлов быстрее, но общий выигрыш atlas достаточен для сохранения текущей модели.
  Упрощать atlas до файлов не следует.

## Затронутые области

| Область | Основные файлы |
|---|---|
| WebP-планировщик | `crates/webp-pipeline/src/{lib,memory,scheduler,tests}.rs` |
| WebP backend | `crates/media/src/animation_backends.rs`, `live_webp.rs`, `recorder.rs` |
| Capture lifecycle | `crates/media/src/recorder.rs`, `recorder/{capture,tests}.rs` |
| Фоновые задачи | `crates/app/src/app/{tasks,state,actions}.rs`, convert и library controllers |
| Профили записи | `crates/core/src/settings.rs`, `crates/app/src/app.rs`, `pages/{record,convert,settings}.rs` |
| WebP options API | `crates/webp-pipeline/src/lib.rs`, `crates/media/src/animation_backends.rs` |
| WebP route | `crates/core/src/settings.rs`, `media/src/recorder{,/request}.rs`, UI record/profile controllers |
| Task policy | `crates/app/src/app/{tasks,actions,state}.rs` и контроллеры one-shot операций |
| Dead/test-only API | `crates/media/src/preview_cache.rs`, `crates/core/src/settings.rs` |
| Library removal | `crates/app/src/pages/library.rs` и `library/actions.rs` |
| Документация | `README.md`, `workflow/stones.md`, `workflow/practices.md` только при появлении устойчивого знания |

## Порядок работ

1. Удалить `MemoryBudget`, `RunReport` и дублирующий WebP options API.
2. Ввести явную policy повторов WebP и проверить fallback-каскад.
3. Перевести capture на терминальное событие и удалить join-watchdog.
4. Ввести `BackgroundJob<T>` и централизовать `TaskPolicy`.
5. Заменить WebP-route на один persisted enum.
6. Разделить persisted-настройки анимации и транзиентный encoder config.
7. Удалить `PreviewLimits`-only API и неиспользуемый `Settings::save`.
8. Унифицировать library removal jobs.
9. Обновить документацию и удалить ставшие неактуальными тесты и поля.

Каждый этап должен оставлять проект в рабочем состоянии. Следующий этап начинается после
успешной целевой проверки предыдущего.

## Проверка

- `cargo test -p mediaa-webp-pipeline`;
- тест постоянной ошибки backend: одна попытка и немедленный внешний fallback;
- тест resource exhaustion: полная деградация и успешное завершение;
- отсутствие production-потребителей у удаляемых WebP report/options API;
- Windows Job Object stress WebP при 256, 512 и 768 МБ с контролем peak working set;
- тесты capture: ready, success, error, panic, timeout и detach без второго потока;
- тесты `BackgroundJob<T>`: result, panic, cancel, deadline, drop и повторное завершение;
- exhaustive-тест policy для каждого `TaskKind` и quit/tray сценарии;
- roundtrip и UI tests всех трёх WebP routes;
- тесты профилей: roundtrip settings и точное формирование `RecordingRequest` для WebP, GIF и
  видео;
- UI tests single delete, bulk cleanup, partial failure и proxy cleanup;
- целевые тесты изменённых app-контроллеров;
- итоговая проверка `cargo test --workspace`;
- итоговая проверка `cargo clippy --workspace --all-targets -- -D warnings`;
- ручная запись WebP с обычным планом, memory-pressure и неподвижным сегментом;
- проверка сохранения исходного MKV при окончательном отказе WebP.

## Риски

- Неизвестная ошибка FFmpeg, которая раньше лечилась дроблением, теперь сразу уйдёт в линейный
  fallback. Это сохраняет результат, но может изменить время завершения и журнал диагностики.
- Ошибочная миграция one-shot задач может потерять panic/deadline или оставить операцию без
  terminal event. Поэтому миграция выполняется небольшими группами с отдельными тестами.
- Capture-канал должен оставаться ограниченным и вычитываться при остановке, иначе завершение
  потока может заблокироваться на отправке результата.
- Ошибочная `TaskPolicy` может либо не дать приложению закрыться, либо потерять обязательную
  операцию. Policy задаётся явно при регистрации и покрывается exhaustive-тестом.
- Замена двух WebP boolean на enum меняет сериализованные настройки. Поскольку выпусков нет,
  migration не создаётся.
- Удаление `PreviewLimits` фиксирует текущий preview 1080p/30 fps. Другого пользовательского
  требования к этим значениям нет.
- Удаление полей настроек сделает соответствующие старые значения неиспользуемыми. Это
  соответствует правилу проекта об отсутствии обратной совместимости.
- Расчётный WebP-бюджет не заменяет измерение реального процесса; безопасность подтверждается
  stress-сценарием, а не self-accounting.

## Вне рамок

- изменение периодического полного rescan до сетевого замера;
- переработка thumbnail atlas: сравнение выполнено, текущая модель остаётся;
- изменение таймаутов audio, native capture и сохранения настроек;
- изменение форматов результата, fallback-каскада или пользовательской компоновки;
- release-сборка без отдельного запроса.
