# План архитектурных работ для LLM

Цель: завершить типизацию управляющих значений, уменьшить рабочий контекст LLM,
централизовать фоновые операции, укрепить межкрейтовые границы и ускорить цикл правки.
UX и пользовательский формат поставки не меняются.

## Инварианты

* Пользователь получает один `dist/windows/mediaa.exe`; split-runtime не добавляется.
* Обычные проверки и отладка идут в debug; release запускается только по прямому запросу.
* Комментарии в коде запрещены, кроме исключений `CLAUDE.md`.
* Исполнитель не коммитит, не пушит, не запускает release, не меняет git index и не запускает приложение.
* Исполнитель трогает только назначенные файлы. Новый общий API сначала фиксируется в задании.
* Публичные API получают тесты поведения; внутренний split не меняет пользовательские тексты.
* После каждой волны голова запускает fmt, clippy и целевые тесты.

## Критерии готовности

* Типовые production-файлы преимущественно не длиннее 600 строк; крупные тестовые файлы разделены по сценариям.
* В публичных API нет boolean-параметров, скрывающих действие, источник, режим или политику.
* Все фоновые one-shot операции имеют единый lifecycle, отмену и typed terminal event.
* UI различает категории ошибок без разбора `anyhow`-строк.
* Исходный, итоговый, временный и cache-путь нельзя перепутать на межкрейтовой границе.
* Каналы с потенциально неограниченным потоком событий имеют bound, latest-value или явное доказательство конечности.
* Локальная правка типичного файла проверяется через `dev-check` за 1–2 секунды на прогретом кэше.
* `cargo test -p mediaa --lib`, целевые тесты крейтов, workspace tests, clippy и audit проходят.

## Граф работ

| Пакет | Зависит от | Может идти параллельно с |
|---|---|---|
| W1A Управляющие значения | нет | W1B, W1C |
| W1B Split app pages | нет | W1A, W1C |
| W1C Split WebP/tests | нет | W1A, W1B |
| W1D Приёмка волны 1 | W1A, W1B, W1C | нет |
| W2A TaskSupervisor | W1D | W2B |
| W2B Typed errors | W1D | W2A |
| W2C Bounded channels | W2A | W2B |
| W2D Typed paths | W2B | W2C |
| W2E Приёмка волны 2 | W2A, W2B, W2C, W2D | нет |
| W3A Core boundary split | W2E | W3B |
| W3B Unix activation | W2E | W3A |
| W3C App facade | W2E | W3A, W3B |
| W3D LLM map | W3C | W3A, W3B |
| W4A Регрессионный аудит | W3A, W3B, W3C, W3D | нет |
| W4B UI и финальная приёмка | W4A | нет |

## W1A — управляющие значения

| Работа | Файлы | Критерий |
|---|---|---|
| `AppIdentity` | `core/paths.rs`, `platform/startup.rs`, `platform/single_instance.rs`, app startup | имя, mutex и window title имеют одного владельца |
| `RecordingLimits` | `media/recorder*` | timeout, poll interval и связанные пороги передаются как конфиг и подменяются в тестах |
| `PreviewLimits` | `media/preview_cache.rs`, app preview callers | memory, frame и worker limits собраны в одном типе |
| Format invariants | `core/naming.rs` | длина timestamp и формат не могут разойтись незаметно |

## W1B — split app pages

| Работа | Файлы | Критерий |
|---|---|---|
| Convert split | `app/pages/convert.rs` и `pages/convert/` | screen, job state, worker, estimate и settings разделены |
| Record split | `app/pages/record.rs` и `pages/record/` | profile editor, source picker и live status разделены |
| Test split | встроенные крупные тесты этих страниц | тесты следуют за ответственностью модулей |
| API stability | `pages/mod.rs`, callers | пользовательские тексты и поведение не меняются |

## W1C — split WebP и integration tests

| Работа | Файлы | Критерий |
|---|---|---|
| Scheduler split | `webp-pipeline/src/lib.rs`, новые подмодули | scheduler, budget и retry имеют отдельные владельцы |
| Test split | `media/tests/pipeline.rs`, общие fixtures | animation/video/audio/conversion сценарии разделены |
| Public API | `webp-pipeline/tests/public_api.rs` | публичный контракт остаётся покрытым |

## W2A — TaskSupervisor

| Работа | Файлы | Критерий |
|---|---|---|
| Typed task model | app task module | `TaskId`, `TaskKind`, terminal outcome и отмена типизированы |
| One-shot registry | app background callers | panicked/disconnected worker становится событием, не молчаливым зависанием |
| Lifecycle | close/quit/recovery/convert/library | один способ зарегистрировать, завершить и дождаться работы |
| Tests | task module tests | panic, drop без события, отмена и повторный shutdown покрыты |

## W2B — typed errors

| Работа | Файлы | Критерий |
|---|---|---|
| Public errors | media/platform/core public boundaries | наружу уходят `RecordingError`, `ConversionError`, `PlatformError`, `StorageError` |
| App mapping | app controllers/notices | UI выбирает действие по типу ошибки, не по тексту |
| Compatibility | тесты API | implementation может использовать `anyhow` внутри, публичный контракт типизирован |

## W2C — bounded channels

| Работа | Файлы | Критерий |
|---|---|---|
| WebP queues | `webp-pipeline`, `media/live_webp.rs` | работа и результаты ограничены |
| Recorder events | `media/recorder.rs` | progress не накапливается; терминальные события не теряются |
| App workers | convert/library/thumbs/player/viewer | очередь имеет bound или latest-value |
| Backpressure tests | соответствующие tests | медленный потребитель не растёт в памяти без предела |

## W2D — typed paths

| Работа | Файлы | Критерий |
|---|---|---|
| Boundary types | core paths/recording/storage | `SourcePath`, `OutputPath`, `WorkDir`, `MediaDir`, `ProxyPath` |
| Callsites | media public API и app adapters | внутренние вычисления могут оставаться `PathBuf` |
| Validation | tests | типы конвертируются только явными конструкторами |

## W3A — core boundaries

Перед split замеряются incremental `dev-check` и focused tests. Split выполняется только если
он уменьшает повторную компиляцию или даёт устойчивость зависимостей при приемлемой цене API.

| Работа | Файлы | Критерий |
|---|---|---|
| Model crate | workspace, новый `mediaa-model` | geometry/frame/formats/naming не тянут SQLite/render |
| Storage crate | workspace, новый `mediaa-storage` | paths/atomic/library/cache/SQLite изолированы |
| Render boundary | workspace, новый `mediaa-render` или модуль render | annotate/cursor изолированы от storage |
| Compatibility policy | все workspace callers | без compatibility re-export; целевые пути обновляются напрямую |
| Measurement | `workflow/dev-context.md` | до/после записаны для одной типовой правки |

## W3B — Unix activation

| Работа | Файлы | Критерий |
|---|---|---|
| Activation channel | `platform/single_instance.rs`, `platform/lib.rs` | второй запуск передаёт запрос активации |
| Window flow | `app/main.rs`, app facade | существующее окно показывается без создания второго runtime |
| Tests | platform tests | занятый instance, владелец и освобождение покрыты |

## W3C — app facade

| Работа | Файлы | Критерий |
|---|---|---|
| Narrow export | `app/src/lib.rs`, `main.rs` | наружу опубликован вход приложения и необходимые тестам типы |
| Module privacy | app modules | внутренний модуль нельзя импортировать без причины |
| Boundary tests | app tests | публичного API достаточно для startup и UI tests |

## W3D — карта для LLM

| Работа | Файлы | Критерий |
|---|---|---|
| Task map | `workflow/dev-context.md` | таблица «задача → файлы → тест → команда» |
| Fast workflow | scripts/workflow docs | debug-first, focused tests, release только вручную |
| Pitfalls | `workflow/dev-context.md` | статические FFmpeg, linker, platform-live и audit limits |

## W4A — повторный аудит

* Повторить поиск boolean-параметров, неограниченных каналов, ручных thread spawn, строковых ошибок и primitive limits.
* Проверить poison/panic пути, shutdown, recovery, atomic write и повреждение библиотеки.
* Проверить дублирование логики между recording/conversion/recovery/cache.
* `cargo audit --file Cargo.lock --no-fetch --deny unsound --deny yanked`.
* Native FFmpeg/libvpx из `vcpkg` не покрыты RustSec; это отдельный будущий native-audit.

## W4B — финальная приёмка

* `cargo fmt --all -- --check`.
* `cargo clippy --workspace --all-targets -- -D warnings`.
* `cargo test -p mediaa --lib`.
* `cargo test --workspace`.
* 41 Node-тест.
* UI-тесты; до/после снимки ключевого экрана при любом заметном изменении UI.
* Проверить один пользовательский runtime artifact и отсутствие release-запуска без запроса.

## Итог исполнения

| Волна | Результат |
|---|---|
| W1 | Типы identity/limits/paths и timestamp invariants; страницы, WebP scheduler и integration tests разделены |
| W2 | `TaskSupervisor`, bounded channels, typed task/error decisions, source/output/work/cache paths и corruption tests |
| W3 | `storage`/`render` выделены из core; platform не зависит от render и production storage; Windows named-event и Unix socket activation; thin binary; LLM-карта |
| W4 | 263 Rust-теста прошли, 4 desktop-live пропущены; 41 Node; fmt, clippy и RustSec без blocking advisories |

Проверки: `cargo check --workspace --all-targets --profile dev-check` после прогрева —
1,35–4,96 s для типовой правки. Пользовательский release остаётся одним exe; release-сборка
и release-тесты в этой работе не запускались.

Осознанные границы: реализации сохраняют `anyhow::Context` для диагностики, а UI получает
типизированную категорию (`MediaErrorKind`); полная иерархия error-enum для каждого crate
не вводилась без отдельного потребителя. Самые большие оставшиеся файлы — центральный
recorder и файлы с встроенными тестами; навигационная карта компенсирует их размер.
RustSec оставляет четыре `unmaintained`, но не `vulnerability`, `unsound` или `yanked`.
