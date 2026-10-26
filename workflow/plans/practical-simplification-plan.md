# Практическое упрощение MediaA и единая архитектура

Дата: 26.10.2026. Основа: рабочее дерево `ef0a2e5`. Статус: обязательные E1–E7 реализованы. E8/E9 не выполнялись без предусмотренных планом доказательств.

Цель: найти подтверждённую кодом избыточность и подготовить исполнимый план упрощения без ухудшения UI/UX, сохранности результатов и стабильности.

Порядок: независимый анализ кода без чтения четырёх указанных пользователем документов; фиксация исходных выводов; затем отдельная оценка `runtime-complexity-simplification.md`, `library-performance-research.md`, `complexity-audit.md`. `complexity-audit-review.md` не читать на любом этапе.

Области: фоновые операции и shutdown, запись и конвертация, библиотека и кэши, границы крейтов, инфраструктура разработки. Для каждого предложения определить фактическую пользу текущего механизма, удаляемые затраты, сохраняемый контракт и проверку. Непроверенные идеи выделить отдельно.

Результаты: этот план, [оценка прежних предложений](prior-simplification-proposals-assessment.md) и [архитектурное руководство для LLM](../architecture.md). Риски: потеря результата при отмене/выходе, блокировка UI, устаревшие асинхронные результаты, регрессия производительности больших библиотек. Проверка текущей работы — сверка ссылок и утверждений с кодом; Rust и UI не меняются.

## Итоговое решение

Основное облегчение даст устранение двойного владения фоновой работой и одинаковое устройство пользовательских функций внутри `app`. Следом — единые effective-настройки записи, сокращение WebP API/отчётности, удаление join-watchdog и объединение удаления файлов. Менять семь крейтов, SQLite, thumbnail atlas или live WebP для этого не требуется.

Рекомендации ранжированы по ожидаемому эффекту на стоимость изменений, а не по числу удалённых строк. Выигрыш в скорости приложения не заявляется без измерения. Сохраняются UI layout, форматы, локализация, Accessibility, preview/playback, отмена, сохранность результатов и диагностируемые ошибки.

Три существенных уточнения после второго этапа: сравнение atlas/JPEG методически некорректно; удаление MemoryBudget сейчас меняет реально достижимую ветку; код линейного fallback формирует неправильный backend request. Поэтому старые планы не принимаются целиком. Подробные доказательства и арифметический контрпример — в отдельной оценке.

## Зафиксированный независимый вывод — до чтения прежних предложений

Этот раздел записан по коду до открытия трёх документов второго этапа. Дальнейшие уточнения будут отмечены отдельно. Это статический аудит, не результат новых замеров или прогона приложения.

1. **Главное упрощение — один владелец каждой операции.** `app/tasks.rs` хранит handles, cancellation, deadlines, `completed` на 32 результата; одновременно `app/state.rs`, `pages/convert.rs`, `pages/library.rs`, `viewer.rs` хранят свои receivers, TaskId и флаги. `App::active_tasks` вычисляет занятость по feature-state, а `has_active_work` добавляет количество задач реестра. `quit` отдельно перечисляет виды для detach. Полезны фоновая работа, контроль отмены и завершения; избыточен второй независимый каталог жизненного цикла. Цель: локальная владеющая операция, один terminal result и явный shutdown владельца; для одноразовой операции использовать проверяемый на завершение `JoinHandle<Result<T, E>>`, для потока событий — локальный session/service handle. Нельзя просто удалить joins или detach финализатор.
2. **Архитектурная неоднородность увеличивает контекст изменения.** Запись управляется в `app/recording.rs`, конвертация — внутри `pages/convert` с доступом ко всему `App`, библиотека держит index/watcher/scan/cleanup прямо в `LibraryUi`, viewer зависит от глобального supervisor. Это не обоснование переписывать семь крейтов. Нужны одинаковые границы features внутри `app`: feature владеет состоянием и работой, UI рисует его состояние и возвращает локальное действие, `App` связывает features и desktop. Перенос должен удалять сквозные зависимости, а не добавлять service/controller/repository слои поверх прежнего кода.
3. **Лишний поток на ожидание чужого потока.** `media/recorder.rs::join_with_timeout` создаёт `join-watchdog`, канал и вариант `Unavailable` для каждого join. Сам timeout остановки захвата полезен. В уже фоновом координаторе достаточно `is_finished` + короткого sleep до общего deadline и `join` только завершившегося потока. Это сохраняет время ожидания и обработку panic, убирает зависимость остановки от возможности создать ещё один поток. При timeout работа не считается остановленной, work files сохраняются.
4. **Оценка размера конвертации использует менее подходящий механизм, чем существующие preview-workers.** `pages/convert/worker.rs::update_estimate` порождает одноразовую задачу на новый ключ после debounce, `Estimate::drop` только выставляет cancel; реестр держит старую работу до завершения. В проекте уже есть `LatestWorker`, применяемый в player/hover preview. Для estimate достаточно одного активного вычисления и одного заменяемого pending-запроса, generation, debounce и сохранения предыдущей оценки. Нельзя механически переиспользовать worker без проверки доставки terminal/error и поведения при зависшем FFmpeg.
5. **Самописный JS-лексер не имеет потребителя.** `core/tests/no_comments.rs` содержит обработку JS regex, template strings, комментариев и тесты к ней; проверяемый каталог `scripts` не содержит `.js`/`.mjs`. Удалить этот путь и его тесты, сохранив правило отсутствия комментариев и проверки используемых языков. Отмена пользовательского правила не требуется и не предлагается.

Условные кандидаты, ещё не признанные безопасными: отказ от ежеминутного rescan; SQLite → snapshot; atlas → отдельные thumbnails; сокращение WebP retry/split/fallback; удаление clipboard timeout/restart; замена SettingsWriter. Код подтверждает цену этих механизмов, но сам по себе не доказывает отсутствие полезного эффекта. Для atlas/index необходимы замеры, для native clipboard — подтверждение блокирующих сценариев и системной семантики, для WebP — проверка timing/alpha/памяти/отказов.

Границы упрощения: сохранить recovery и публикацию без перезаписи, общий clock записи, аппаратные capture backends и осмысленные fallback, ограниченные очереди кадров, version/generation проверки stale-result. Не дробить `core` ради идеальной чистоты и не сводить все виды работы к универсальному runtime. Проверены исходники основных owners, библиотечного индекса/кэшей, записи/финализации, WebP scheduler, preview, platform capture/clipboard, Cargo-граф и проверки репозитория; все platform backends вживую не проверялись.

## Подтверждённая избыточность и выбранные упрощения

### C1. Два владельца фоновой операции и несколько моделей занятости

- **Решение:** `app/src/app/tasks.rs:95` хранит TaskId, handle, cancel, deadline и completed history; `app/state.rs`, `pages/convert.rs`, `pages/library.rs`, `viewer.rs` отдельно хранят receiver/TaskId/cancel. `app/actions.rs::has_active_work` объединяет feature-state с размером registry, `quit` держит ручной detach-list.
- **Текущая польза:** фоновые операции не блокируют интерфейс; приложение замечает panic, ограничивает отдельные ожидания, удерживает важную работу при выходе. Эти обязанности нужны.
- **Неоправданные затраты:** terminal result и завершение потока обрабатываются разными owners; history сохраняет результаты для отстающего потребителя; при добавлении операции приходится менять несколько каталогов. Большинство потребителей игнорируют возвращаемый TaskExit.
- **Предлагаемое упрощение:** локально владеющий `Job<T>` для конечной операции; session/service handle для потоковой; занятость и выход вычислять из конкретных owners. После перевода потребителей удалить TaskId, TaskKind, completed-map, ActiveTasks и ручной список detach.
- **Обоснование:** потребителю нужен собственный результат и состояние своего ресурса; общего UI списка задач или внешнего API registry нет. Владение не исчезает, а становится единственным. Не добавлять BackgroundJob поверх постоянного registry.
- **Проверка:** result/error/panic, cancel-before-start и во время работы, поздний успех после cancel, повторный poll, смена страницы, quit во время save/conversion/recovery. Конечный результат не теряется; UI не делает blocking join.

### C2. Разные границы одного и того же пользовательского сценария

- **Решение:** запись управляется `app/recording.rs`, конвертация — `pages/convert/worker.rs` через весь App, библиотека хранит lifecycle в `LibraryUi`, viewer принимает TaskSupervisor. UI-функции систематически получают `&mut App`.
- **Текущая польза:** это прямые вызовы с небольшим числом абстракций; такой подход сам по себе допустим.
- **Неоправданные затраты:** изменение одной операции требует понимать App и внутренности страниц; state, запуск, UI и shutdown разбросаны. Это наблюдаемая связанность, а не нарушение формального архитектурного шаблона.
- **Предлагаемое упрощение:** четыре owners — recording, capture, conversion, library — внутри `app/src/features`; UI получает свой owner/настройки и возвращает локальные действия; App координирует переходы между ними. Перенести существующие поля и методы, удалив прежние сквозные обращения.
- **Обоснование:** единая граница сокращает контекст изменения для LLM и позволяет выполнить C1. Не создавать дополнительных repository/service/view-model слоёв и не переносить код ради симметрии дерева.
- **Проверка:** feature UI не принимает весь App, operations продолжают poll на скрытой странице, App не читает внутренние rx/task поля, прежние UI/interaction tests проходят без массового обновления baseline.

### C3. Persisted FPS/height дублируют effective-параметры записи

- **Решение:** `core/src/settings.rs::Settings` хранит RecordSettings и AnimSettings для трёх профилей; `app.rs:555` перезаписывает animation FPS/height перед записью.
- **Текущая польза:** полный AnimSettings удобен энкодеру и конвертации; качество анимации действительно отличается от видеокачества.
- **Неоправданные затраты:** сохранены два значения там, где UI редактирует одно, лишние defaults/sanitization/roundtrip и код перезаписи.
- **Предлагаемое упрощение:** persisted animation quality без FPS/height; итоговый encoder config строить из профиля один раз. Runtime WebP mode выводить из preferences один раз, убрать дублирующее переименование AnimationPath там, где оно обозначает тот же mode.
- **Обоснование:** удаляются значения, не управляющие записью. При этом предпочтение parallel у отключённого segmented checkbox сохраняется; отдельный Direct/Segmented тип текущей encode-попытки нужен fallback. Три persisted варианта не заменяют эти разные смыслы без потери поведения.
- **Проверка:** App/Hotkey/Region × video/GIF/WebP, roundtrip новых настроек, FPS/height фактического request, выключение и повторное включение segmented с сохранением parallel preference, независимость conversion settings.

### C4. WebP отчётность и два одинаковых набора tuning-полей

- **Решение:** `webp-pipeline/src/lib.rs` дублирует поля в PipelineTuning и PipelineConfig, строит RunReport/Degradation; `media/src/animation_backends.rs` превращает `Ok(_)` в `Ok(())` и извлекает только error.source.
- **Текущая польза:** параметры позволяют управлять concurrency, проводить failure/stress tests и benchmark; trace помогает разбирать ошибки.
- **Неоправданные затраты:** production не читает возвращаемые отчёты, часть состояния обслуживает только эти структуры; копирование полей создаёт дополнительные места синхронизации.
- **Предлагаемое упрощение:** один вложенный tuning, `run → Result<(), PipelineError>` с причиной/отменой; полезные summary/trace оставить в логах; plan сделать внутренним, удалить неиспользуемые поля отчётности и builder-дубли.
- **Обоснование:** результат и диагностика сохраняются; не нужно возвращать накопленную историю без читателя. Возможность задать memory/workers для существующих benchmark и интеграционных тестов остаётся.
- **Проверка:** проверять выходные файлы, количество вызовов fake backend, порядок/длительность кадров, cancel и failure; собрать также examples/public_api. Не заменять смысловые тесты assert по строкам логов.

### C5. Дополнительный поток для join с timeout

- **Решение:** `media/src/recorder.rs:177` создаёт join-watchdog, bounded channel, TimedJoin::Unavailable; `reap_capture` и shutdown используют его для уже существующего capture handle.
- **Текущая польза:** не ждать бесконечно native capture; общий capture_stop_timeout оправдан.
- **Неоправданные затраты:** при зависании остаются два потока; завершение capture зависит от успешного создания нового потока. Канал повторяет результат, уже доступный через JoinHandle.
- **Предлагаемое упрощение:** проверять is_finished до общего deadline в фоновом координаторе, коротко спать между проверками, join только завершившегося handle. Удалить watchdog/channel/Unavailable, сохранить небольшой тип результата ожидания, если он делает match яснее.
- **Обоснование:** срок ожидания и обнаружение panic сохраняются без второго потока. Полное удаление любого enum ожидания не является целью.
- **Проверка:** normal/error/panic/timeout, несколько capture под одним deadline, отмена startup, отсутствие cleanup рабочего каталога живого timed-out worker, сохранение доступных исходников.

### C6. Дублирование single-delete и bulk cleanup

- **Решение:** `app/src/pages/library.rs::{start_cleanup,start_delete,poll_cleanup}` повторяют занятость, подготовку viewer, spawn и terminal обработку; single-delete конструирует фиктивный CleanupPlan::Week, а cleanup_success хранится отдельно.
- **Текущая польза:** два нужных сценария и корректная повторная проверка возраста для cleanup.
- **Неоправданные затраты:** общий lifecycle написан дважды, часть результата зависит от отдельного несвязанного поля; fictitious age скрывает реальное намерение.
- **Предлагаемое упрощение:** одна операция удаления с request `Selected` или `OlderThan`, одним job и относящимся к нему текстом результата. Общая подготовка/poll; разную проверку файлов оставить явной.
- **Обоснование:** сокращается механизм, не число пользовательских возможностей; partial failure и связь с previews/proxies сохраняются.
- **Проверка:** single delete, bulk age, файл изменился после подтверждения, часть trash-вызовов завершилась ошибкой, открытый viewer, invalidation thumbnails/proxy, повторное удаление и exit.

### C7. Отдельный поток на каждую новую оценку conversion

- **Решение:** `pages/convert/worker.rs::update_estimate` создаёт новую задачу после debounce при изменении key; Drop Estimate выставляет cancel, старую работу продолжает учитывать registry.
- **Текущая польза:** оценка не тормозит UI; старый ответ не заменяет новый, пользователь видит previous estimate во время расчёта.
- **Неоправданные затраты:** отдельные id/rx/deadline и потенциально одновременно завершающиеся старые jobs для replaceable вычисления. В player и hover уже применяется latest-worker.
- **Предлагаемое упрощение:** использовать существующий bounded latest-request механизм: один активный estimate, один pending, generation; debounce и previous value оставить. Терминальная ошибка/timeout принадлежит conversion owner.
- **Обоснование:** расчёт не изменяет пользовательский результат и старые запросы не нужны. Переход допустим лишь при проверенной своевременной кооперативной отмене; если она не работает, этап ограничить объединением состояния job, без потери отзывчивости новой оценки.
- **Проверка:** быстрые изменения input/quality, задержанный старый ответ, заполненная result queue, error/panic, отмена, уход со страницы, запуск настоящей conversion. Число активных estimate вычислений ограничено.

### C8. Проверки и API без нужного потребителя

- **Решение:** `core/tests/no_comments.rs:202` и далее содержат JS lexer с regex/template strings; сканируются `.js/.mjs` в scripts, которых там нет. `Settings::save()` не имеет вызовов; публичный PreviewLimits протянут через wrapper при постоянных production limits.
- **Текущая польза:** правило комментариев автоматизировано; сохранение настроек и пределы preview нужны.
- **Неоправданные затраты:** поддержка неиспользуемого языка и внешних вариантов API увеличивает контекст без текущего сценария.
- **Предлагаемое упрощение:** удалить JS parser/его тесты и ветку обхода; сохранить проверки используемых языков и само правило. Удалить Settings::save; упростить limits helper до текущих 1080p/30 fps, сохранив ensure_proxy_for и реализацию encode.
- **Обоснование:** это узкая очистка без изменения пользовательского поведения. Test-only seam не удаляется автоматически: если он нужен для meaningful failure/performance tests, он остаётся приватным или явно ограниченным API.
- **Проверка:** inventory scripts, no_comments test, поиск вызовов, settings roundtrip, preview dimension/rate tests и сборка examples.

## Гипотезы и ограничения решений

| ID | Кандидат | Что установлено | Решение сейчас и условие пересмотра |
|---|---|---|---|
| H1 | Atlas → отдельные thumbnail files | Текущий benchmark сравнивает разные pixels и buffered/unbuffered I/O | Решение пользователя: оставить atlas |
| H2 | Отказ от периодического полного rescan | Локальный эксперимент подтвердил низкую цену scan, но обнаружил UI-задержку apply_scan и необходимость fallback при overflow | Решение: оставить reconciliation и периодический rescan; отдельно оптимизировать применение 50k snapshot |
| H3 | Удаление live WebP | Это заметная подсистема, сокращающая post-stop работу | Решение пользователя: оставить live WebP |
| H4 | Удаление MemoryBudget | Проверка 288 конфигураций не нашла aggregate-only превышений поверх worker_limit, но 82 конфигурации получили сегмент сверх бюджета из-за округления | Guard бюджета полезен; CAS-reservation можно заменить только точным preflight-расчётом и split до запуска |
| H5 | Полный отказ от retry/split | Известные постоянные ошибки не лечатся повтором; неизвестные могут зависеть от ресурсов | Реализована typed классификация pipeline: permanent normalize/validate/panic завершаются сразу, budget resource и unknown сохраняют bounded retry. Неизвестные FFmpeg-ошибки не классифицируются по тексту |
| H6 | Удаление clipboard restart/timeouts или упрощение SettingsWriter до sync save | Native зависание и медленный диск имеют реальные последствия | Не включать без воспроизводимого безопасного эквивалента; оставить текущие owners |
| H7 | SQLite → snapshot/JSON | Нет сравнения равных сценариев load/query/incremental update/recovery | Решение пользователя: оставить SQLite |

Оправданные механизмы здесь не записаны как «находки сложности». OS backends, frame pacing, ограниченные очереди, recovery, no-clobber publish, invalidation hits/details и независимые source abstractions остаются условиями качества. Собственный WebP merge не заменять библиотекой без совпадения alpha/timing/merge semantics.

### Результаты проверки H2 и H4

26.10.2026, локальный Windows/NTFS, release-free debug benchmark:

| Сценарий | Результат |
|---|---|
| H2 scan, 10k | p50 18,4 ms, p95 19,5 ms, apply_scan 5,4 ms |
| H2 scan, 50k | p50 157,3 ms, p95 186,7 ms, apply_scan 54,3 ms |
| H2 watcher, 2k create | 2000/2000 путей, overflow=false, missed=0 |
| H2 burst, 50 delete + 50 create | overflow=true, targeted refresh 15,7 ms, fallback scan+apply 152,2/62,9 ms, stale/missing после rescan = 0 |
| H4 grid, 288 configs | 82 конфигурации с сегментом сверх бюджета; 0 конфигураций, где каждый worker укладывается отдельно, но conservative worker_limit допускает превышение суммы |
| H4 4K60 q100, 3 s, 512 MiB | округлённый сегмент 59 кадров превышает бюджет; runtime guard останавливает выполнение до публикации файла |

H2 вывод: periodic rescan пока не удалять. Его background scan стоит около 0,16 s на 50k раз в минуту; риск заметного поведения создаёт синхронный apply_scan на UI, а не сам scan. Overflow recovery уже работает своевременно. Для решения об отказе от periodic rescan нужны подтверждения отсутствия silent event loss на всех поддерживаемых FS/ОС.

H4 вывод: удалять весь budget нельзя. Per-segment guard защищает от реального округления и oversized segmentation. При этом CAS-reservation не даёт дополнительной защиты для обычного агрегата поверх worker_limit в проверенной сетке; его допустимо заменить preflight-проверкой и split до запуска только вместе с общим точным расчётом.

## План перехода

Это дизайн исполнения, а не разрешение на изменение Rust-кода. При начале реализации создать в `workflow/tasks.md` конкретные задачи этапа с четырьмя полями и выполнять по одной. Этап с несколькими независимыми изменениями разбить на такие задачи. Не превращать условные исследования в обязательный объём первой реализации.

Каждый этап оставляет приложение работоспособным. Новые пути из architecture.md — целевые; пока модуль не перенесён, менять его существующий путь. В рамках одной операции менять владельца целиком, без compatibility re-exports и двух параллельных реализаций для неё.

### E0. Зафиксировать поведение и убрать ложные предпосылки

**Действия и файлы:**

1. Проверить актуальную ревизию, изменения пользователя и известные успешные прогоны. Использовать существующие `app/ui_tests.rs`, `app/test_support.rs`, `pages/*/tests.rs`, `media/tests/pipeline` как основу. Добавлять только отсутствующие контрактные сценарии для следующего этапа; не переписывать весь test harness.
2. Перед E8 проверить реальный fallback: `media/recorder.rs:883` игнорирует аргумент попытки и выбирает segmentation из исходного режима. Сделать выбор backend зависимым от текущей попытки и добавить тест, проходящий построение настоящего request/выбор backend, с принудительным отказом segmented и успешным direct. Не ограничиваться тестом `encode_with_linear_fallback` с подставной closure. Проверить cancel и MKV recovery при отказе обеих попыток.
3. Перед решением H1 исправить `storage/examples/library_bench.rs`: RGBA → RGB по пикселям; одинаковая буферизация записи; явный flush в измерении; фиксировать encode и write отдельно, сравнивать одинаковые pixels до lossy encoding. Старые коэффициенты 188×/8,5× пометить невалидными в исследовании. Выполнение большого benchmark нужно только для решения H1, не для C1–C8.
4. Для достоверной non-Windows проверки отдельно устранить `platform/single_instance.rs:240` с отсутствующим development_scope и проверить правильный scope activation на целевой ОС. Это дефект платформенной сборки, не находка сложности; не расширять его до переработки single-instance.

**Готово:** известна исходная проверенная версия; дефекты и их проверки не смешаны с косметическим переносом. Пункты 2–4 являются локальными предпосылками соответствующих веток, а не общим блокирующим gate для всего плана. Недоступный Linux/macOS runner честно отмечается, не выдаётся за успешную проверку.

### E1. Очистить очевидные лишние API и проверки

**Зависимость:** исходное состояние из E0. **Находки:** C4, C8.

**Действия:** удалить JS часть no_comments; Settings::save; упростить публичный limits wrapper preview без изменения параметров. В отдельной задаче вложить PipelineTuning в PipelineConfig, убрать RunReport/Degradation/estimated_bytes и методы, дублирующие доступ к tuning. Сохранить memory/workers настройки используемых benchmark, trace и typed cancellation; обновить примеры и integration tests. Не менять retry и MemoryBudget в этой задаче.

**Файлы:** `core/tests/no_comments.rs`, `core/src/settings.rs`, `media/src/preview_cache.rs`, `webp-pipeline/src/{lib,scheduler,tests}.rs`, `webp-pipeline/tests/public_api.rs`, `media/src/animation_backends.rs`, `media/examples/animation_bench.rs` и найденные вызовы.

**Проверка/готово:** пользовательские output/preview параметры прежние; JS-потребителя нет; нет старых символов; tests проверяют поведение, а не удалённую отчётность; examples и all-targets compile. Peak accounting, обслуживающий только удалённый report, можно убрать, но активное ограничение reserve пока сохранить.

### E2. Сделать настройки записи однозначными

**Зависимость:** E1 удобен, но не обязателен. **Находка:** C3.

**Действия:** выделить persisted AnimationQuality с gif_colors/gif_dither/webp_quality; оставить fps/output_height в RecordSettings. Обновить defaults/sanitized/profile/profile_editor, собрать полный AnimSettings/request в одном месте. Удалить перезапись cloned settings из `recording_animation_settings`. Ввести единственное преобразование preferences → runtime WebpMode; общий тип может жить в core, потому что он нужен и app, и media. Убрать лишний AnimationPath как переименование режима, но оставить явный тип encode-попытки Direct/Segmented.

**Файлы:** `core/src/settings.rs`, `app/src/app.rs`, `app/src/app/recording.rs`, `app/src/pages/{record,settings,convert}.rs`, `media/src/recorder{,/request}.rs`, соответствующие tests. Профильные defaults и UI editor требуют согласованного изменения.

**Проверка/готово:** по одному persisted источнику FPS/height для каждого профиля, реальный request соответствует видимым полям; disabled parallel preference переживает toggle и save/load. Conversion не меняется. Legacy schema migration и aliases не добавлять согласно CLAUDE.md; проверить загрузку нового TOML и ожидаемые defaults при несовместимом старом конфиге, не скрывать этот эффект.

### E3. Убрать join-watchdog

**Зависимость:** только подходящие capture tests E0. **Находка:** C5.

**Действия:** локально заменить реализацию `join_with_timeout` на ожидание is_finished с коротким sleep, ограниченным оставшимся временем, в уже фоновом координаторе. Общий deadline shutdown не превращать в новый полный timeout на каждый capture. Обработку `Finished/Panicked/TimedOut` оставить явной, удалить Unavailable и каналы вспомогательного join-thread. Пройти все startup-failure и normal-shutdown callers.

**Файлы:** `media/src/recorder.rs`, `recorder/tests.rs`, при необходимости `recorder/capture.rs` для существующих failure-path вызовов.

**Проверка/готово:** тест `capture_join_timeout_returns_without_waiting_for_a_stuck_worker` и error/panic/multi-capture сценарии; ни UI blocking join, ни новый event protocol не добавлены. Timeout не удаляет ресурсы живого worker. Зависший native поток сам по себе этим изменением не устраняется.

### E4. Первый feature owner и локальный Job

**Зависимость:** E0 baseline. **Находки:** C1, C2, C7.

**Действия:** перенести ConvertUi runtime state и `pages/convert/worker.rs` в `features/conversion`; сохранить поля формы, не копировать их в новую view-model. Добавить небольшой `jobs.rs::Job<T>` с owned handle и poll. Сначала перевести ConversionProbe, затем применить тот же контракт к LibraryLoad/LibraryScan в их текущем владельце. Эти три операции полностью перестают регистрироваться в supervisor.

После проверки перевести пользовательскую conversion на локальное владение. Сохранить progress/cancel и commit результата. Объединить estimate-состояние, затем перевести его на LatestWorker при успешной проверке cancellation; generation/debounce/previous estimate остаются. Не сериализовать пользовательскую conversion за долгим estimate в одной очереди.

**Файлы:** `app/src/pages/convert{,/worker,/controls,/tests}.rs`, `app/src/pages/library.rs`, новый `features/conversion`, `jobs.rs`, `latest_worker.rs`, `app.rs`.

**Проверка/готово:** скрытая conversion продолжает работать; старый probe/estimate не применяется к новому input; terminal/panic не теряются; slow cancel не размножает workers и не лишает новую оценку прежней отзывчивости. Существует два реальных Job-потребителя без общего registry. Если LatestWorker не обеспечивает эквивалентность при blocking FFmpeg, этот подпункт остаётся условным, остальные переводы принимаются отдельно.

### E5. Перенести recording и capture с полным владением

**Зависимость:** E3 и проверенный Job E4. **Находки:** C1, C2.

**Действия:** переместить RecState, start/finalize handles, polling и команды в RecordingController. CaptureController получает pending/hide-delay, capture/overlay/screenshot jobs. Разделить короткие команды shell (показать/скрыть окно, перейти в библиотеку) и работу владельца. Не отдавать финализатор в глобальный реестр: Saving удерживает его до фактического завершения. Сохранить session events для warning/progress/failure; их смысл не дублировать one-shot terminal result.

UI записи и аудио получает recording owner/настройки/catalog, возвращает действия; screenshot UI/overlay работает через capture owner. UI/tray/hotkey вызывают одинаковые методы. Обновить test_support конструктор без реальных OS-сервисов.

**Файлы:** `app/src/app/{state,recording,capture,actions}.rs`, `app/src/pages/{record,audio,capture}.rs`, overlay, новые `features/recording`, `features/capture`, `app.rs` и UI tests.

**Проверка/готово:** старт/stop во время Starting, pause/resume, save/discard, скриншот save+copy с частичной ошибкой, переключение страницы, послеоперационные уведомления/звук, quit. Нет регистрации перенесённых операций в supervisor и нет параллельной копии RecState. Screenshot save при выходе удерживается до безопасного результата — это явно закреплённый целевой контракт.

### E6. Перенести library/viewer и объединить removal

**Зависимость:** E4. **Находки:** C1, C2, C6.

**Действия:** переместить index/watcher/load/scan/reconciliation/query/hits в LibraryController, а UI/layout — в соседний ui-модуль. Viewer/player/previews/thumbs сгруппировать рядом с реальным потребителем; viewer image/audio open перевести на локальные jobs. Убрать входной параметр TaskSupervisor у viewer.

Оставить один метод запуска removal с enum Selected/OlderThan. Метаданные запроса и success message хранить вместе с job; результат содержит report, а не фиктивную неделю. Сохранить повторную age-проверку bulk cleanup, подготовку viewer/previews и invalidation после частичного успеха. App принимает только внешние LibraryAction: open/reveal/convert/copy/rename.

**Файлы:** `app/src/pages/library{,/actions,/cleanup,/grid,/tests}.rs`, `viewer.rs`, `player.rs`, `hover_preview.rs`, `thumbs.rs`, новый `features/library`, `app.rs`. `storage/src/library_index.rs` менять лишь при необходимости узкого API, не переписывать storage.

**Проверка/готово:** смена каталога во время load/scan, watcher events во время scan, details обновляют hits, поиск/сортировка/подгрузка, single/bulk/partial deletion, viewer seek/close/audio. Не появляется второй индекс, полный clone на кадр или потеря быстрого cache path. Производительность apply_scan остаётся отдельной H2, перенос файла не выдаётся за её исправление.

### E7. Завершить shell/services и удалить registry

**Зависимость:** E4–E6. **Находки:** C1, C2.

**Действия:** каталог устройств и desktop open/reveal/rename получают конкретных owners, startup recovery — явное владение в App или recording owner по одному выбранному месту. Перенести существующие ClipboardService/SettingsWriter в services без переписывания внутреннего протокола. Settings/about/shell UI получают узкие данные и действия, без доступа к rx/handles других features.

Реализовать App shutdown как вызовы begin_shutdown/poll конкретных владельцев и вычисляемое условие завершения. Разделить blocks_exit и tray display. Удалить TaskSupervisor, TaskId, TaskKind, completed history, ActiveTasks/WorkKind, detach-list и импорты из страниц. Удалить test `direct_threads_are_limited_to_owned_long_lived_workers`, если он требует старую топологию; заменить нужную гарантию тестами владения/выхода, не новым парсером архитектуры.

**Файлы:** `app/src/app/{tasks,actions,desktop,recovery,state}.rs`, `app.rs`, catalog/settings/clipboard services, UI shell/settings/about, tests.

**Проверка/готово:** exit матрица из architecture.md; clipboard busy не выдаёт запись за Saving; поздний native ответ не меняет новый контекст; последняя версия настроек сохраняется. Поиск старых символов пуст; feature UI не принимает App; доступ к внутренним полям заменён конкретным API, а не новым global context.

### E8. Сократить повторы WebP, которые не меняют причину отказа

**Зависимость:** E0 fallback исправлен и проверен; E1/E2 API согласованы. **Кандидат:** подтверждённый unconditional retry, безопасный охват ограничен H5.

**Действия:** обозначить известную постоянную ошибку normalize/validate и typed resource failure на границе backend. Permanent failure и panic не прогонять шесть раз через split; отмена никогда не запускает fallback. Для подтверждённого resource failure оставить ограниченную деградацию workers/segment size. Для неизвестной FFmpeg ошибки сохранить прежнее bounded поведение до воспроизводимого решения; не писать string classifier.

Конкретная encode-попытка передаётся в AnimationEncodeRequest независимо от выбранного режима сессии. Сохранить live → offline segmented → direct → сохранение исходников; не обещать, что OOM-abort процесса будет пойман как ошибка.

**Файлы:** `webp-pipeline/src/{lib,scheduler,tests}.rs`, `media/src/animation_backends.rs`, `media/src/recorder.rs`, pipeline integration tests.

**Проверка/готово:** fake permanent backend вызывается один раз; typed resource failure деградирует и может восстановиться; неизвестный сбой имеет явную проверенную политику; real request выбирает direct после segmented failure; cancel, static segments, alpha, duration и recovery сохраняются. Если upstream не позволяет классификацию, не расширять изменение на неизвестные ошибки.

26.10.2026. Безопасная часть реализована: `SegmentFailure::{Retryable, Permanent}` отделяет известное исчерпание бюджета от permanent normalize/validate и panic. Permanent и panic больше не запускают новый split; resource и unknown сохраняют прежние ограниченные попытки. Реальный FFmpeg Internal/OOM остаётся unknown, пока adapter не предоставит достоверный typed resource signal. Тесты pipeline, recorder fallback и cancel проходят.

### E9. Удалить динамический бюджет только после доказательства эквивалентности

**Зависимость:** E1/E8; **условный этап H4**, не обязательный результат первой реализации.

**Действия:** сначала единый расчёт количества кадров/стоимости между plan и round, включая microsecond rounding, последний короткий сегмент и retry split. Можно хранить frame count в Segment либо вычислять консервативную верхнюю границу одним helper; выбрать вариант с меньшим числом преобразований. Перед каждым round проверить вместимость отдельного сегмента и ограничить workers по максимальной стоимости pending-сегментов. Не маскировать невместимый сегмент `max(1)`.

После доказанного инварианта `workers × max_segment_cost <= effective_budget` убрать CAS/reservations и невозможную после предварительного расчёта ветку. Если инвариант без значительной новой сложности получить не удаётся, сохранить нынешнюю защиту и зафиксировать отказ от этой части упрощения.

**Файлы:** `webp-pipeline/src/{memory,lib,scheduler,tests}.rs`, при изменении contract — media adapter/examples.

**Проверка/готово:** автоматические boundary tests fps 1–60, tiny/exact budgets, min/max frames, split, последний сегмент; отдельный случай 3840×2160/60/quality100/512 MiB из оценки. Сравнить реальный RSS и выход до/после на тех же fixtures. Жёсткие memory-limit stress tests выполнять в отдельном собственном процессе; учитывать native allocator/process abort. Удалённый CAS не заменяется более сложным allocator/framework.

Предварительная проверка 288 конфигураций не нашла aggregate-only превышений поверх `worker_limit`, но нашла 82 конфигурации с rounded segment сверх бюджета. Поэтому E9 не выполнен: guard остаётся, CAS-reservation — только кандидат на замену после точного preflight и отдельного RSS A/B.

### E10. Сверить итог и обновить документацию

**Зависимость:** принятые обязательные этапы завершены; условные имеют явное решение.

**Действия:** обновить README map и фактические пути architecture.md, удалить ставшие неиспользуемыми locale keys/tests/API, проверить обратные зависимости. Отразить только подтверждённые новые факты в stones/practices. Не записывать «MemoryBudget удалён» или «atlas доказанно лучше», если соответствующий этап не состоялся. В tasks статусы должны описывать реально выполненную работу.

**Готово:** критерии ниже выполнены; не осталось временного слоя поверх старого supervisor и обещаний о ещё не проверенной платформе. Если отказались от H1–H7, это не мешает завершению подтверждённых упрощений.

## Проверка реализации и критерии приёмки

Команды выполнять в настроенном окружении из README. После изменения Rust: быстрый `cargo check --workspace --lib --bins --profile dev-check`, затем целевые tests. Для финального состояния — обязательные `cargo test --workspace` и `cargo clippy --workspace --all-targets` (можно `-- -D warnings` по существующему test.ps1), formatting check. Не повторять успешные проверки без изменения кода; validation records привязать к проверенной версии.

| Изменение | Минимальная смысловая проверка |
|---|---|
| E1 | `cargo test -p mediaa-core --test no_comments`; settings/preview tests; `cargo test -p mediaa-webp-pipeline`; all-targets включает examples |
| E2 | core settings tests, app record/profile UI tests, request validation media |
| E3 | `cargo test -p mediaa-media recorder::tests`; startup/capture/pipeline tests с корректной сериализацией настоящих sessions |
| E4–E7 | controller lifecycle tests и `cargo test -p mediaa --lib app::ui_tests` (после переноса — актуальный module path); targeted conversion/library tests |
| E8/E9 | webp-pipeline tests, media `--test pipeline`, реальный stop-to-file/recovery и ограниченный stress |
| UI/архитектура | Старые AccessKit labels, settings/record/audio/convert/library interaction, responsive, Russian/light/DPI snapshots на Windows |

После переноса namespaces использовать реальные новые имена тестов; приведённые команды не разрешают молча выполнить ноль тестов. Pixel baselines не обновлять автоматически ради прохождения проверки. UI/UX-эквивалентность включает быстрый отклик при старте/отмене, непрерывный progress, работу в скрытом окне, уведомления и отсутствие потери состояния при навигации.

Для реальной UI-проверки — отдельная тестовая библиотека `MEDIAA_DEV_LIBRARY`, уникальный `MEDIAA_INSTANCE_ID`, собственный PID и snapshot только своего окна. Пользовательский экземпляр не трогать. Для чисто документационного аудита Rust suite и UI-запуск не нужны и в этой работе не выполнялись.

Обязательный итог: один owner операции, один terminal result, локальные инварианты и сохранённые пользовательские сценарии. Признаки неудачного упрощения: выросло число синхронизируемых state-полей, UI ждёт диск/native join, terminal outcome можно потерять, новый helper требует общей регистрации или ради удаления защиты создана более сложная система. В таком случае пересмотреть конкретный этап, не продолжать миграцию автоматически.

## Что именно проверено в этом аудите

Статически прослежены основные production-вызовы и тестовые seams в app, storage, media, webp-pipeline, core и выбранных platform-модулях; прочитаны Cargo-манифесты и команды проверок. Подтверждено отсутствие JS/mjs в проверяемом scripts, отсутствие вызовов Settings::save и production-читателей RunReport, реальные wrappers preview, построение fallback request и граница RGB/RGBA benchmark. Для спорного reserve выполнено арифметическое воспроизведение формул, приведённое в оценке.

Это не исчерпывающий аудит каждой строки и не live-проверка всех capture backends. Новых цифр производительности, успешной Linux/macOS сборки или пройденных Rust-тестов здесь не заявляется. Исполнимость плана обеспечивают конкретные owners/files/contracts/gates; безопасность самих изменений должна быть подтверждена при реализации.
