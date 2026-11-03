# Прогресс

Одна строка — одна задача. Статусы: `ожидание`, `в работе`, `готово`, `отказ`.
Строки DONE-* и REJ-* перенесены из `feature/agent-temp` (`workflow/done.md`): задачи, закрытые и отклонённые до 25.10.26.

| ID | Дата и время | Описание | Статус | Контекст |
|---|---|---|---|---|
| DONE-1 | 15.10.26 | Агент: фазы P0–P5 | готово | — |
| DONE-2 | 15.10.26 | Проверка моделей: папка без GGUF — ошибка | готово | — |
| DONE-3 | 20.10.26 | Промпты вынесены из кода в шаблоны | готово | — |
| DONE-4 | 20.10.26 | Документы задач | готово | — |
| DONE-5 | 20.10.26 | Подагенты на DeepSeek через OpenRouter (бывшая задача 11.1) | готово | — |
| DONE-6 | 20.10.26 | Вкладки открытых файлов фиксированной ширины (бывшая 7.1) | готово | — |
| DONE-7 | 20.10.26 | Рабочий процесс с подагентами | готово | — |
| DONE-8 | 20.10.26 | Движок инференса отдельно от приложения (бывшие 4.3 E1, 4.4 E2) | готово | — |
| DONE-9 | 21.10.26 | Темы видео (бывшая 6.1) | готово | — |
| DONE-10 | 21.10.26 | Чаты вкладками (бывшая 7.2) | готово | — |
| DONE-11 | 21.10.26 | Ссылки на логи подагентов не ломаются при архивации | готово | — |
| DONE-12 | 21.10.26 | Темы видео: правки интерфейса после просмотра пользователем | готово | — |
| DONE-13 | 21.10.26 | Конвейер анализа, вся работа в очереди, разбор тем, чат как в агентных системах (бывшие 11.1, 11.2, 6.3, 7.4, 7.5, 3.9, 1.2) | готово | — |
| DONE-14 | 21.10.26 | Grok — третий исполнитель `scripts/subagent.sh` (бывшая 12.1) | готово | — |
| DONE-15 | 21.10.26 | Статистика расхода по исполнителям (бывшая 12.2) | готово | — |
| DONE-16 | 21.10.26 | Анализ падал на распознавании; понятные ошибки и скачивание моделей (бывшая 7.4) | готово | — |
| DONE-17 | 22.10.26 | Голова — Grok (бывшая todos 14.1) | готово | — |
| DONE-18 | 22.10.26 | Правки пользователя после проверки (бывший раздел 13 todos) | готово | — |
| DONE-19 | 22.10.26 | Evals: минимальная версия и первый замер (часть todos 9) | готово | — |
| DONE-20 | 23.10.26 | Никаких комментариев в коде; «почему» — в проверяемом виде (решение пользователя) | готово | — |
| DONE-21 | 23.10.26 | Диаризация, автоименование, поиск агента, язык, steer, вызовы за шаг (бывшие 14.4, 14.6, 1.4, 2.1, 3.1–3.3, 3.5–3.7) | готово | — |
| DONE-22 | 23.10.26 | Сверка списка задач: найдено сделанным (бывшая 9.6) | готово | — |
| DONE-23 | 23.10.26 | Очередь заново, модели в памяти, правки интерфейса (бывшие 14.2, 14.3, 14.5, 14.7–14.19) | готово | — |
| DONE-24 | 24.10.26 | Зеркало подагентов в OpenCode: умирает вместе с приложением, история находится после переезда | готово | — |
| DONE-25 | 25.10.26 | Агент вынесен в ветку feature/agent, из main удалён полностью | готово | — |
| REJ-1 | — | Переход на llama.cpp «для универсальности форматов» (20) — снято пользователем | отказ | — |
| REJ-2 | — | Очереди и задачи на Temporal (19) — нужен отдельный сервер, а приложение должно быть одним exe и работать офлайн | отказ | — |
| REJ-3 | — | Встроенный (вшитый в приложение) движок как отдельный режим — движок ставит пользователь | отказ | — |
| REJ-4 | — | Модели распознавания onnx-asr, faster-whisper (CTranslate2) и whisper.cpp (GGML) — другой препроцессинг, конфликт ggml с llama.cpp | отказ | — |
| REJ-5 | — | Безголовый браузер для веб-поиска — тяжело и ломает «один exe» | отказ | — |
| REJ-6 | — | Подагент (`delegate`, роль «Подагент») — удалён 23.10.26, извлечение покрывают разбор тем и скан | отказ | — |
| SK-1 | 25.10.26 23:39 | Создать вызываемый skill `$mediaa-audit` | готово | [repository-maintenance.md](tasks/repository-maintenance.md) |
| SK-2 | 25.10.26 23:39 | Создать вызываемый skill `$mediaa-complexity` | готово | [repository-maintenance.md](tasks/repository-maintenance.md) |
| SK-3 | 25.10.26 23:39 | Подключить skills в Claude Code без дублирования инструкций | готово | [repository-maintenance.md](tasks/repository-maintenance.md) |
| SK-4 | 25.10.26 23:39 | Включить обнаружение project skills в VS Code | готово | [repository-maintenance.md](tasks/repository-maintenance.md) |
| MEM-1 | 25.10.26 23:51 | Создать проектную память агентов | готово | [documentation-restructure.md](tasks/documentation-restructure.md) |
| MEM-2 | 25.10.26 23:51 | Зафиксировать иерархию источников и правила безопасности | готово | [documentation-restructure.md](tasks/documentation-restructure.md) |
| REL-1 | 25.10.26 23:56 | Зафиксировать правила повторного тестирования | готово | [repository-maintenance.md](tasks/repository-maintenance.md) |
| UI-REG-1 | 26.10.26 00:20 | Добавить `egui_kittest` fixtures и app-level smoke tests | готово | [ui-regression.md](tasks/ui-regression.md) |
| UI-REG-2 | 26.10.26 00:20 | Добавить interaction/geometry tests ключевых сценариев | готово | [ui-regression.md](tasks/ui-regression.md) |
| UI-REG-3 | 26.10.26 00:20 | Добавить Windows snapshot baselines | готово | [ui-regression.md](tasks/ui-regression.md) |
| UI-REG-4 | 26.10.26 00:20 | Покрыть screenshot fullscreen/window/region | готово | [ui-regression.md](tasks/ui-regression.md) |
| UI-REG-5 | 26.10.26 00:20 | Покрыть UI video/audio и stop из окна/трея | готово | [ui-regression.md](tasks/ui-regression.md) |
| UI-REG-6 | 26.10.26 00:20 | Покрыть settings, library и conversion UI | готово | [ui-regression.md](tasks/ui-regression.md) |
| UI-REG-7 | 26.10.26 00:20 | Проверить responsive layouts | готово | [ui-regression.md](tasks/ui-regression.md) |
| UI-REG-8 | 26.10.26 00:20 | Проверить WebP fallback notice и trace | готово | [ui-regression.md](tasks/ui-regression.md) |
| UI-REG-9 | 26.10.26 00:20 | Проверить single-instance isolation | готово | [ui-regression.md](tasks/ui-regression.md) |
| UI-REG-10 | 26.10.26 01:32 | Покрыть region annotation/save/cancel | готово | [ui-regression.md](tasks/ui-regression.md) |
| UI-REG-11 | 26.10.26 01:32 | Покрыть recording pause/resume/cancel/save | готово | [ui-regression.md](tasks/ui-regression.md) |
| UI-REG-12 | 26.10.26 01:32 | Проверить DPI, Russian UI и light theme | готово | [ui-regression.md](tasks/ui-regression.md) |
| PERF-1 | 26.10.26 02:09 | Добавить scan/index и thumbnail benchmarks | готово | [performance.md](tasks/performance.md) |
| PERF-2 | 26.10.26 02:09 | Прогнать local library/thumbnail scenarios | готово | [performance.md](tasks/performance.md) |
| PERF-3 | 26.10.26 02:09 | Запустить большую MediaA и измерить startup/CPU/RSS/rescan | готово | [performance.md](tasks/performance.md) |
| PERF-4 | 26.10.26 02:09 | Зафиксировать результаты performance hypotheses | готово | [performance.md](tasks/performance.md) |
| PRACT-1 | 26.10.26 03:04 | Независимо проверить избыточную сложность | готово | [architecture-simplification.md](tasks/architecture-simplification.md) |
| PRACT-2 | 26.10.26 03:04 | Подготовить план упрощений и руководство для LLM | готово | [architecture-simplification.md](tasks/architecture-simplification.md) |
| ARCH-1 | 26.10.26 03:47 | Удалить неиспользуемые API и WebP report | готово | [architecture-simplification.md](tasks/architecture-simplification.md) |
| ARCH-2 | 26.10.26 03:47 | Разделить persisted quality и effective recording settings | готово | [architecture-simplification.md](tasks/architecture-simplification.md) |
| ARCH-3 | 26.10.26 03:47 | Убрать join-watchdog | готово | [architecture-simplification.md](tasks/architecture-simplification.md) |
| ARCH-4 | 26.10.26 03:47 | Создать Job и ConversionController | готово | [architecture-simplification.md](tasks/architecture-simplification.md) |
| ARCH-5 | 26.10.26 03:47 | Перевести library load/scan на локальное владение | готово | [architecture-simplification.md](tasks/architecture-simplification.md) |
| ARCH-6 | 26.10.26 03:47 | Создать RecordingController | готово | [architecture-simplification.md](tasks/architecture-simplification.md) |
| ARCH-7 | 26.10.26 03:47 | Создать CaptureController | готово | [architecture-simplification.md](tasks/architecture-simplification.md) |
| ARCH-8 | 26.10.26 03:47 | Создать LibraryController и перенести viewer lifecycle | готово | [architecture-simplification.md](tasks/architecture-simplification.md) |
| ARCH-9 | 26.10.26 03:47 | Объединить single-delete и bulk cleanup | готово | [architecture-simplification.md](tasks/architecture-simplification.md) |
| ARCH-10 | 26.10.26 03:47 | Завершить shell/services и удалить TaskSupervisor | готово | [architecture-simplification.md](tasks/architecture-simplification.md) |
| ARCH-11 | 26.10.26 03:47 | Обновить карту путей и провести итоговые проверки | готово | [architecture-simplification.md](tasks/architecture-simplification.md) |
| HYP-1 | 26.10.26 04:46 | Проверить периодический rescan | готово | [performance.md](tasks/performance.md) |
| HYP-2 | 26.10.26 04:46 | Проверить MemoryBudget | готово | [performance.md](tasks/performance.md) |
| HYP-3 | 26.10.26 04:46 | Зафиксировать H1/H3/H7 и объяснить H5/H6 | готово | [performance.md](tasks/performance.md) |
| HYP-4 | 26.10.26 04:58 | Классифицировать WebP permanent/resource retry | готово | [architecture-simplification.md](tasks/architecture-simplification.md) |
| SPEC-1 | 26.10.26 05:24 | Заменить `workflow` на `specs` и синхронизировать правила, архитектуру, ограничения и task context | готово | [documentation-restructure.md](tasks/documentation-restructure.md) |
| SPEC-2 | 26.10.26 05:29 | Удалить дубли правил из `CLAUDE.md` и оставить его точкой входа к `specs/` | готово | [claude-deduplication.md](tasks/claude-deduplication.md) |
| REL-2 | 26.10.26 05:30 | Проверить документацию и UI, создать коммит и выполнить push | готово | [repository-maintenance.md](tasks/repository-maintenance.md) |
| UI-NOTICE-1 | 26.10.26 05:38 | Добавить анимацию уведомлений, гарантированный auto-hide, silent actions и выравнивание header | готово | [notification-polish.md](tasks/notification-polish.md) |
| UI-REGION-1 | 26.10.26 05:53 | Разрешить annotations и text вне selection и показывать их без clip во время редактирования | готово | [region-annotations-outside-selection.md](tasks/region-annotations-outside-selection.md) |
| UI-REGION-2 | 26.10.26 06:20 | Добавить рамку, drag и Shift+Enter; убрать смещение после commit и подложку | готово | [region-text-frame-drag.md](tasks/region-text-frame-drag.md) |
| UI-REGION-3 | 26.10.26 06:30 | Исправить исчезновение overlay после Tab и зависание region action | готово | [region-tab-monitor-switch.md](tasks/region-tab-monitor-switch.md) |
| UI-REGION-4 | 26.10.26 06:33 | Добавить UX-тесты repeated Tab и стабильного multi-monitor переключения | готово | [region-tab-monitor-ux-tests.md](tasks/region-tab-monitor-ux-tests.md) |
| UI-REGION-5 | 26.10.26 06:54 | Убрать артефакты первого показа overlay через opaque pre-show viewport | готово | [region-overlay-smooth-open.md](tasks/region-overlay-smooth-open.md) |
| UX-AUDIT-1 | 26.10.26 07:00 | Закрыть приоритетные пробелы UX-тестов dynamic overlay, populated pages и responsive layout | готово | [ux-coverage-gaps.md](tasks/ux-coverage-gaps.md) |
| UI-REGION-6 | 26.10.26 07:18 | Не допускать перекрытия selection size badge и активного text frame | готово | [region-size-badge-collision.md](tasks/region-size-badge-collision.md) |
| SITE-1 | 26.10.26 07:24 | Создать маркетинговый сайт MediaA и Pages-деплой через GitHub Actions | готово | [site-marketing.md](tasks/site-marketing.md) |
| AGENT-PORT-1 | 27.10.26 20:20 | Довести перенос агента до сборки, тестов и архитектурного соответствия main | готово | [agent-port.md](tasks/agent-port.md) |
| BUILD-1 | 28.10.26 11:44 | Вернуть Windows-сборке генератор VS и static CRT toolchain, чтобы `run.ps1` снова собирал и запускал приложение | готово | [windows-build.md](tasks/windows-build.md) |
| RUN-1 | 28.10.26 13:40 | Запускать собранный MediaA копией вне репозитория, чтобы SQLite в библиотеке открывался | готово | [dev-run-outside-workspace.md](tasks/dev-run-outside-workspace.md) |
| AGENT-EXT-1 | 28.10.26 13:11 | Убрать встроенный инференс (llama.cpp, Vulkan, sherpa-onnx) и перевести модели агента на внешние подключения | готово | [agent-external-steps.md](tasks/agent-external-steps.md) |
| AGENT-GEN-1 | 28.10.26 19:10 | Поля модели: контекст, размышления, флаги речи, диаризация ролика выключена по умолчанию | готово | [agent-genai.md](tasks/agent-genai.md) |
| AGENT-GEN-2 | 28.10.26 19:10 | Перевести чат и эмбеддинги на genai, реранк и проверку имени оставить своими | готово | [agent-genai.md](tasks/agent-genai.md) |
| AGENT-GEN-3 | 28.10.26 19:10 | Убрать команду и отдельный OpenAI, добавить провайдера расшифровки | готово | [agent-genai.md](tasks/agent-genai.md) |
| AGENT-GEN-4 | 28.10.26 19:10 | Контракт расшифровки: время слов, спикеры, отпечаток и два запроса OpenAI | готово | [agent-genai.md](tasks/agent-genai.md) |
| AGENT-GEN-5 | 28.10.26 19:10 | Убрать стадию «Говорящие» и склеивать слова со спикерами в одной расшифровке | готово | [agent-genai.md](tasks/agent-genai.md) |
| AGENT-GEN-6 | 28.10.26 19:10 | Экран моделей: контекст, глубина размышлений, галочки речи | готово | [agent-genai.md](tasks/agent-genai.md) |
| AGENT-GEN-7 | 28.10.26 19:10 | Анализ ролика: диаризация выключена, пока её не включили, и молчит при выключенной модели | готово | [agent-genai.md](tasks/agent-genai.md) |
| AGENT-GEN-8 | 28.10.26 19:10 | Обновить архитектуру, ограничения и руководство под genai и одну расшифровку | готово | [agent-genai.md](tasks/agent-genai.md) |
| AGENT-GEN-9 | 28.10.26 19:10 | Полный прогон тестов и проверка diff после AGENT-GEN-8 | готово | [agent-genai.md](tasks/agent-genai.md) |
| LIMIT-1 | 28.10.26 22:35 | Группы лимитов вместо бюджета и вкладка «Параметры» | готово | [limit-groups.md](tasks/limit-groups.md) |
| LIMIT-2 | 29.10.26 01:55 | Пулы по умолчанию пусты, в интерфейсе «группа» переименована в «пул» | готово | [limit-groups.md](tasks/limit-groups.md) |
| SHOT-1 | 29.10.26 02:10 | Скриншот области: выбор папки и формата в системном диалоге, оверлей закрывается после записи | готово | [screenshot-save-dialog.md](tasks/screenshot-save-dialog.md) |
| HK-1 | 29.10.26 02:20 | Глобальные горячие клавиши выключены по умолчанию | готово | [hotkeys-off-by-default.md](tasks/hotkeys-off-by-default.md) |
| SHOT-2 | 29.10.26 02:35 | Убрать опции «После снимка» и скрывать окно с задержкой перед снимком области | готово | [screenshot-options-removed.md](tasks/screenshot-options-removed.md) |
| REC-1 | 29.10.26 02:50 | Завершённая запись всегда копируется в буфер и попадает в библиотеку; выбор убран | готово | [recording-completion-always-copies.md](tasks/recording-completion-always-copies.md) |
| TRAY-1 | 29.10.26 03:00 | «При запуске открывать в трее» доступно только при автозапуске | готово | [tray-start-requires-autostart.md](tasks/tray-start-requires-autostart.md) |
| TRAY-2 | 29.10.26 03:10 | Галочка «Скрывать в трей» только в записи из программы, с проверкой задержки и скрытия | готово | [hide-to-tray-option-scope.md](tasks/hide-to-tray-option-scope.md) |
| SHOT-3 | 29.10.26 03:25 | «Копировать» и Enter в region overlay всегда сохраняют в библиотеку и копируют в буфер | готово | [copy-always-saves.md](tasks/copy-always-saves.md) |
| UI-2 | 29.10.26 03:40 | Контракт расшифровки открывается из меню провайдеров, кнопка со страницы «О программе» убрана | готово | [transcription-contract-in-providers.md](tasks/transcription-contract-in-providers.md) |
| TOOL-1 | 29.10.26 03:55 | run.ps1 сам сбрасывает кэш ffmpeg при сбое, снимает блокировку скриптов; добавлен run.cmd | готово | [self-healing-run-script.md](tasks/self-healing-run-script.md) |
| TRAY-3 | 29.10.26 19:30 | Настройка «Показывать прогресс в трее», по умолчанию включена | готово | [tray-progress-toggle.md](tasks/tray-progress-toggle.md) |
| UI-3 | 30.10.26 14:40 | UI-снимок области и выбор области записи возвращают окно после оверлея; трей и хоткей — нет | готово | [ui-capture-restores-window.md](tasks/ui-capture-restores-window.md) |
| TRAY-4 | 30.10.26 14:55 | Запись экрана и аудио из трея не скрывает и не показывает окно | готово | [tray-recording-keeps-window.md](tasks/tray-recording-keeps-window.md) |
| TRAY-5 | 30.10.26 15:10 | Любое действие трея, кроме «Открыть окно», не меняет состояние окна | готово | [tray-never-touches-window.md](tasks/tray-never-touches-window.md) |
| REC-2 | 30.10.26 15:25 | Во время записи цели и настройки видны, но неактивны; иконка трея не показывает capture | готово | [recording-ui-lock.md](tasks/recording-ui-lock.md) |
| UI-4 | 30.10.26 15:45 | Открытая модалка блокирует мышь, клавиатуру и уведомления, не затемняя фон | готово | [modal-input-block.md](tasks/modal-input-block.md) |
| REC-3 | 30.10.26 16:05 | Устранено зависание параллельного сегментированного WebP при сохранении (bounded result channels) | готово | [webp-live-result-deadlock.md](tasks/webp-live-result-deadlock.md) |
| REC-4 | 30.10.26 16:25 | Фикстуры-видео, тесты 3 мониторов/2 окон, per-source live fallback, закрытие окна, linear fallback | готово | [webp-multi-source-tests.md](tasks/webp-multi-source-tests.md) |
| REC-5 | 30.10.26 16:40 | Preview для WebP строится из финального файла, без ошибок NAL при чтении промежуточного MKV | готово | [preview-proxy-nal-noise.md](tasks/preview-proxy-nal-noise.md) |
| UI-5 | 30.10.26 16:55 | Просмотрщик декодирует с backpressure, без CPU-шторма и выброшенных кадров | готово | [viewer-frame-backpressure.md](tasks/viewer-frame-backpressure.md) |
| UI-6 | 30.10.26 17:10 | WebP/GIF миниатюры и плеер не ждут MP4-прокси, генерация в фоне | готово | [lazy-animation-thumbnails.md](tasks/lazy-animation-thumbnails.md) |
| UI-7 | 30.10.26 17:25 | Полные hover-превью через лёгкий 360p/15fps прокси, без декода оригинала | готово | [hover-preview-proxy.md](tasks/hover-preview-proxy.md) |
| UI-8 | 30.10.26 17:45 | Настройки анимированных превью и полных видео-превью, очистка кэша при выключении | готово | [preview-options.md](tasks/preview-options.md) |
| UI-9 | 30.10.26 18:05 | Фоновая очистка preview-прокси и атласов миниатюр старше 7 дней | готово | [cache-maintenance.md](tasks/cache-maintenance.md) |
| UI-10 | 30.10.26 18:20 | Тесты настроек превью, hover, backpressure и очистки кэшей | готово | [preview-coverage.md](tasks/preview-coverage.md) |
| UI-11 | 30.10.26 18:35 | Удаление обычного и hover-кэша вместе с медиафайлом, тест на сбой перемещения | готово | [delete-media-cache.md](tasks/delete-media-cache.md) |
| UI-12 | 30.10.26 18:55 | Анимированный индикатор генерации превью в карточках и просмотрщике | готово | [preview-loader.md](tasks/preview-loader.md) |
| UI-13 | 30.10.26 19:10 | Возврат окна после записи зависит от источника старта, не от источника остановки | готово | [recording-stop-window-source.md](tasks/recording-stop-window-source.md) |
| REC-6 | 30.10.26 19:30 | Общий старт capture-потоков и финальная обрезка выходов под минимальную длительность | готово | [multi-source-duration.md](tasks/multi-source-duration.md) |
| UI-14 | 30.10.26 20:10 | `unused`/`dead_code` запрещены на уровне workspace, все crate наследуют lint-настройки | готово | [deny-unused.md](tasks/deny-unused.md) |
| REC-8 | 30.10.26 23:45 | Старые скиллы удалены; добавлен mediaa-video-tests с метриками, таблицей и ссылками на документы | готово | [video-tests-skill.md](tasks/video-tests-skill.md) |
| REC-9 | 31.10.26 00:35 | Разрешение записи свёрнутых окон и отключённых мониторов, идентичность окна, deferred-источник, тесты | готово | [unavailable-sources.md](tasks/unavailable-sources.md) |
| UI-15 | 31.10.26 00:45 | «Скрывать в трей при закрытии окна» выключено по умолчанию | готово | [close-to-tray-default.md](tasks/close-to-tray-default.md) |
| UI-16 | 31.10.26 00:55 | Строки окон в Capture такие же кликабельные, как строки дисплеев | готово | [capture-window-rows.md](tasks/capture-window-rows.md) |
| UI-17 | 31.10.26 01:05 | Постоянная кнопка «Отмена» в overlay выбора области | готово | [region-cancel-button.md](tasks/region-cancel-button.md) |
| UI-18 | 31.10.26 01:10 | Убрана подпись под кнопкой «Начать запись» | готово | [record-start-no-hint.md](tasks/record-start-no-hint.md) |
| REC-7 | 30.10.26 23:20 | TV fake, настройки, 30-секундные MP4-сценарии на разных размерах и сравнительные MP4-документы в target (не закоммичено) | тесты | [mp4-video-snapshots.md](tasks/mp4-video-snapshots.md) |
| REC-10 | 31.10.26 03:10 | Удалены сегментированный и live-WebP; Direct Quality/Fast, Fastest method 0; Fast в среднем 2,3× быстрее, полный экран 4,1–4,5× | готово | [webp-speed-presets.md](tasks/webp-speed-presets.md) |
| UI-19 | 31.10.26 11:15 | Единые пресеты записи: видео 3, анимация 2, аудио 3; Custom виртуальный; настройки хранятся отдельно по форматам | готово | [webp-speed-presets.md](tasks/webp-speed-presets.md) |
| ARCH-12 | 31.10.26 11:15 | Созданы mediaa-ffmpeg, mediaa-webp, mediaa-gif; webp-pipeline и zengif удалены | готово | [format-crates.md](tasks/format-crates.md) |
| GIF-1 | 31.10.26 03:10 | Замерен текущий GIF и zengif; zengif на полном экране в 2–7× медленнее, не внедрён | отказ | [webp-speed-presets.md](tasks/webp-speed-presets.md) |
| CONV-1 | 31.10.26 14:50 | Пакетная конвертация: мультивыбор и drag&drop, матрица целей, sequential queue, retry, отмена, без оценки, UI snapshots | готово | [multi-file-conversion.md](tasks/multi-file-conversion.md) |
| LIB-1 | 31.10.26 14:50 | Массовые операции: выделение и день, copy/convert/delete в корзину/rename, отчёт без retry, snapshot | готово | [library-bulk-actions.md](tasks/library-bulk-actions.md) |
| ARCH-13 | 31.10.26 13:15 | FFmpeg/GIF/WebP закрыты форматными крейтами; `media` больше не импортирует `ez-ffmpeg` и `image-webp` | готово | [format-crates.md](tasks/format-crates.md) |
| SET-1 | 31.10.26 13:20 | Простые/расширенные настройки: простой режим по умолчанию, сброс скрытых полей, snapshots и запись | готово | [simple-advanced-settings.md](tasks/simple-advanced-settings.md) |
| PLAT-1 | 31.10.26 13:40 | Чистая граница платформы: скрыты OS-модули, добавлены audio-фабрики и WindowResult, архитектурный тест | готово | [platform-boundary.md](tasks/platform-boundary.md) |
| DOC-UI-1 | 01.11.26 01:35 | Описать reusable widgets, темы и пути пользовательских настроек в архитектуре | готово | [ui-docs-widgets-themes-settings.md](tasks/ui-docs-widgets-themes-settings.md) |
| PLAT-2.1 | 01.11.26 04:12 | Каркас `windows/` и перенос screen/capture/WGC | готово | [platform-layout.md](tasks/platform-layout.md) |
| PLAT-2.2 | 01.11.26 04:12 | Window/outline: перенос Win32 и `bool` → `WindowResult` | готово | [platform-layout.md](tasks/platform-layout.md) |
| PLAT-2.3 | 01.11.26 04:12 | Cursor/keys/single_instance/startup/audio в `windows/` | готово | [platform-layout.md](tasks/platform-layout.md) |
| PLAT-2.4 | 01.11.26 04:12 | `platform::init()` и DPI awareness в примерах | готово | [platform-layout.md](tasks/platform-layout.md) |
| PLAT-2.5 | 01.11.26 04:12 | Unsupported-политика: ошибки экрана и уведомления | готово | [platform-layout.md](tasks/platform-layout.md) |
| PLAT-2.6 | 01.11.26 04:12 | Boundary-тест и финальные проверки | готово | [platform-layout.md](tasks/platform-layout.md) |
| PLAT-2.7 | 01.11.26 04:47 | Документация: архитектура, правила и раскладка платформенного кода | готово | [platform-layout.md](tasks/platform-layout.md) |
| PLAT-2.8 | 01.11.26 05:44 | Разложить non-Windows файлы по `linux/` и `unix/` | готово | [platform-layout.md](tasks/platform-layout.md) |
| ARCH-14 | 01.11.26 06:10 | Согласовать разделение `app` на `shell`, `logic`, `ui` и `adapters` и порядок этапов | готово | [ui-logic-separation.md](tasks/ui-logic-separation.md) |
| ARCH-14.1 | 01.11.26 06:10 | Зафиксировать правила слоёв и проверку границ | готово | [ui-logic-separation.md](tasks/ui-logic-separation.md) |
| ARCH-14.2 | 01.11.26 06:10 | `Intent` на малых страницах, снять `Deref`, `AgentServices` в `logic` | готово | [ui-logic-separation.md](tasks/ui-logic-separation.md) |
| ARCH-14.3 | 01.11.26 06:10 | `WindowPresence` и чистые решения capture/recording в `logic` | готово | [ui-logic-separation.md](tasks/ui-logic-separation.md) |
| ARCH-14.4 | 01.11.26 06:10 | Разделить overlay на `logic/region_editor` и `ui/region_editor` | готово | [ui-logic-separation.md](tasks/ui-logic-separation.md) |
| ARCH-14.5 | 01.11.26 06:10 | Разделить notices, вынести рендеринг из контроллеров, `edit_settings` | готово | [ui-logic-separation.md](tasks/ui-logic-separation.md) |
| ARCH-14.6 | 01.11.26 06:10 | Перенести эффекты `pages/agent` в `adapters` | готово | [ui-logic-separation.md](tasks/ui-logic-separation.md) |
| ARCH-15.1 | 01.11.26 08:40 | Убрать TextureHandle из logic/library, воркеры декодирования в логику | готово | [ui-logic-separation.md](tasks/ui-logic-separation.md) |
| ARCH-15.2 | 01.11.26 09:25 | Разделить hotkeys на адаптер и UI-захват | готово | [ui-logic-separation.md](tasks/ui-logic-separation.md) |
| ARCH-16 | 01.11.26 08:55 | Перенести UI-регресс тесты из logic в ui и проверять тестовый код гейтом слоёв | готово | [ui-logic-separation.md](tasks/ui-logic-separation.md) |
| ARCH-17 | 01.11.26 09:05 | Аудит размещения тестов по слоям ui, shell, dapters и общий тестовый 	est_support в корне | готово | [ui-logic-separation.md](tasks/ui-logic-separation.md) |
| ARCH-18 | 01.11.26 09:42 | Убрать &mut App из ui: страницы на Ctx и Intent, запрет crate::shell в ui | готово | [ui-logic-separation.md](tasks/ui-logic-separation.md) |
| ARCH-19 | 01.11.26 09:55 | mediaa-arch: allowlist зависимостей между крейтами и запрет зависимости от app | готово | [crate-boundaries.md](tasks/crate-boundaries.md) |
| ARCH-20 | 01.11.26 10:05 | mediaa-arch: feature 	esting только в dev, mediaa_engine::testing только в тестах | готово | [crate-boundaries.md](tasks/crate-boundaries.md) |
| UI-20 | 01.11.26 10:20 | Разрешение видео хоткея и трея видно в простых настройках, включая кастомное значение | готово | [hotkey-resolution-simple.md](tasks/hotkey-resolution-simple.md) |
| UI-21 | 01.11.26 10:32 | Стрелка разворота уведомления направлена по фактическому раскрытию вверх | готово | [notice-toggle-direction.md](tasks/notice-toggle-direction.md) |
| UI-22 | 01.11.26 10:40 | Конвертация из карточки и выделения библиотеки не переключает на вкладку «Конвертировать» | готово | [library-convert-no-navigation.md](tasks/library-convert-no-navigation.md) |
| UI-23 | 01.11.26 10:52 | Скриншот недоступного дисплея или окна не делается, показывается предупреждение | готово | [screenshot-unavailable-source.md](tasks/screenshot-unavailable-source.md) |
| UI-24 | 01.11.26 11:05 | Битый файл не останавливает пакетную конвертацию, причина видна в подсказке статуса | готово | [conversion-batch-file-failures.md](tasks/conversion-batch-file-failures.md) |
| UI-25 | 01.11.26 11:18 | Уведомления включены по умолчанию и после сброса; отправка в конвертацию показывает уведомление с именами файлов | готово | [notifications-default-and-convert-queue.md](tasks/notifications-default-and-convert-queue.md) |
| UI-26 | 01.11.26 11:30 | Исчезновение верхнего уведомления не сдвигает стек: кадр удаления сразу перерисовывается | готово | [notice-stack-position.md](tasks/notice-stack-position.md) |
| UI-27 | 01.11.26 11:38 | Снапшоты стека уведомлений: два, после закрытия, после истечения | готово | [notice-stack-position.md](tasks/notice-stack-position.md) |
| PERF-5 | 02.11.26 00:40 | Разобрать простой: 440 MB RSS и периодическая нагрузка CPU, отделить графику от логики | готово | [idle-memory-cpu.md](tasks/idle-memory-cpu.md) |
| PERF-6 | 02.11.26 00:50 | Добавить аудит памяти и CPU по состояниям приложения с baseline-снимком и сравнением | готово | [performance.md](tasks/performance.md) |
| PERF-7 | 02.11.26 01:05 | Шаг 0: проверить бэкенд wgpu замерами и функциональными сценариями | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-1 | 02.11.26 01:05 | Согласовать контракт сплита: роли, владение, IPC, жизненный цикл, прогресс | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-2 | 02.11.26 01:05 | Каркас IPC на interprocess: handshake с версией, framing, права, обрыв | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-3 | 02.11.26 01:05 | Роли запуска --daemon/--ui и лаунчер, single-instance, автозапуск | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.1 | 02.11.26 01:50 | Трей, хоткеи и жизненный цикл в демоне; UI закрывается в трей, скрытый UI выходит через 3 с | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.2 | 02.11.26 01:50 | Настройки: владение демоном, снапшот в UI, запись через SettingsWriter | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.3a | 02.11.26 02:05 | Закрытие при активной работе скрывает окно; скрытый UI выходит после завершения | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.3b1 | 02.11.26 04:20 | Recovery в демоне: старт, StartRecovery, снапшот в UI | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.3b2a | 02.11.26 04:30 | Эффекты записи: старт через host, RecordingEffect, единый apply | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.3b2b | 02.11.26 04:40 | Эффекты записи: poll событий и завершение через host | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.3b2c1 | 02.11.26 04:55 | RecordUi вынесен из контроллера в UI-состояние ViewState | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.3b2c2a | 02.11.26 05:10 | Снапшот записи: serde RecordingView, ViewState::apply | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.3b2c2b | 02.11.26 05:25 | Протокол записи и daemon-движок: команды, снапшоты, события | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.3b2c3 | 02.11.26 05:40 | UI переключён на демон: команды, снапшоты, локальный контроллер только в тестах | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.3b2d | 02.11.26 05:55 | Проверка записи в демоне без UI: record-smoke.ps1, изоляция dev-библиотеки, замер | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.3b2 | 02.11.26 05:40 | Запись в демоне: эффекты, протокол, снапшоты | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.3c | 02.11.26 04:25 | Снимки: захват остаётся в UI (окно, скрытие, оверлей), демон только просит | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.4a | 02.11.26 06:05 | Watcher библиотеки в демоне: LibraryChanged, UI применяет paths/rescan | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.4b | 02.11.26 06:35 | Индекс библиотеки в демоне: скан/запись, UI reload-only | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-4.4 | 02.11.26 01:50 | Индекс библиотеки и watcher в демоне, миниатюры в UI (детали пока пишет UI) | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-5 | 02.11.26 07:10 | UI: запись и библиотека на снапшотах, выход по закрытию, выход при потере демона | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-6 | 02.11.26 07:10 | Прогресс/статусы: запись снапшотами, конвертация и агент опросом при открытом окне | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-7a | 02.11.26 06:55 | Агент: демон поднимает AgentHost при закрытом UI, роняет при подключении | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-7 | 02.11.26 07:10 | Агент: handoff в демон при закрытом UI, host в UI при открытом | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-8 | 02.11.26 04:12 | Эфемерные уведомления: ring с TTL в демоне, отображение в UI | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-9a | 02.11.26 02:20 | Замер задержки пикера и ускорение холодного старта (без FFmpeg и лишней работы) | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-9b | 02.11.26 02:20 | Маска/нативный слой на время холодного старта пикера (цель < 400 мс) | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-9c | 02.11.26 02:58 | Фоновый режим тестовых окон: ничего не всплывает, фокус остаётся у пользователя | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-9d | 02.11.26 03:15 | Скрипты Windows разложены по подпапкам build/run/test/checks, README для человека и AGENTS.md для агента | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-9e | 02.11.26 03:25 | Скрипты скриншотов в checks: list-windows, capture-screen, capture-window | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-9f | 02.11.26 03:40 | Тестовый ввод: MEDIAA_DEV_INPUT + send-input.ps1 (клавиши, указатель, клик, драг) | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-9g | 02.11.26 04:00 | Правила платформы в rules.md, правила агента в scripts/windows/AGENTS.md, красная иконка копии агента | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-9h | 02.11.26 04:05 | Правила: убраны скиллы, закрытые задачи не редактируются, правило перечитывания при сжатии контекста | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-9i | 02.11.26 04:06 | AppMode: единая тестовая конфигурация, dev-чтения убраны из UI/демона | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-10 | 02.11.26 02:07 | Логи двух процессов, слот последней ошибки, версия протокола | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-11 | 02.11.26 04:35 | Тесты сплита: handshake, отказ, обрыв, падение job, UI-тесты и слой-гейт | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-12 | 02.11.26 07:10 | Документация, architecture/constraints, таблица замеров | готово | [background-first.md](tasks/background-first.md) |
| PERF-8 | 02.11.26 01:10 | Зафиксировать политику бэкенда: GL по умолчанию с fallback и настройкой | ожидание | [deferred.md](tasks/deferred.md) |
| AGENT-1 | 02.11.26 07:15 | Агент без UI-хоста: Live/Feed по IPC, демон владеет агентом всегда | ожидание | [deferred.md](tasks/deferred.md) |
| THUMB-1 | 02.11.26 07:15 | Детали миниатюр через демон: SetDetails вместо записи из UI | ожидание | [deferred.md](tasks/deferred.md) |
| FIX-2 | 02.11.26 07:35 | UI застревал в «Сохранение записи»: демон не слал финальный Idle-снапшот | готово | [background-first.md](tasks/background-first.md) |
| AGENT-ICON-1 | 02.11.26 07:25 | mediaa-agent.exe с красной иконкой в exe: Task Manager, Explorer, build-agent.ps1 | готово | [background-first.md](tasks/background-first.md) |
| FIX-1 | 02.11.26 07:05 | Tray-команда терялась в settings handshake: буферизация, тест, dev-хук MEDIAA_DEV_TRAY | готово | [background-first.md](tasks/background-first.md) |
| SPLIT-13 | 02.11.26 01:31 | Закрытие приложения: Quit демону, принудительное завершение фоновых задач, тест | готово | [background-first.md](tasks/background-first.md) |
| SYNC-1 | 02.11.26 07:45 | Крейт mediaa-sync: кадры, версия протокола, length-prefix, JSON-кодек, тесты | готово | [sync.md](tasks/sync.md) |
| SYNC-2 | 02.11.26 07:45 | StateHub/StateView/EventLog: версии, coalescing, seq, gap | готово | [sync.md](tasks/sync.md) |
| SYNC-3 | 02.11.26 08:05 | Server/Client state machines: subscribe/snapshot/changed/resync/command/event | готово | [sync.md](tasks/sync.md) |
| SYNC-4a | 02.11.26 08:25 | Settings: стартовый снимок и изменения через store | готово | [sync.md](tasks/sync.md) |
| SYNC-4b | 02.11.26 08:40 | Settings: команда SetSettings с ack, старый SettingsSync удалён | готово | [sync.md](tasks/sync.md) |
| SYNC-5a | 02.11.26 08:55 | Recording: состояние через store, версии, финальный Idle | готово | [sync.md](tasks/sync.md) |
| SYNC-5b | 02.11.26 09:15 | Recording: команды start/stop/pause/cancel через Command | готово | [sync.md](tasks/sync.md) |
| SYNC-5c | 02.11.26 09:15 | Recording: события finished/window/files через EventLog | готово | [sync.md](tasks/sync.md) |
| SYNC-6 | 02.11.26 09:30 | Library: store `library`, reload-only, coalescing загрузок | готово | [sync.md](tasks/sync.md) |
| SYNC-7 | 02.11.26 09:50 | Notices: ring-store с id; recovery: state + report event | готово | [sync.md](tasks/sync.md) |
| SYNC-8 | 02.11.26 10:15 | Tray/hotkeys — события, tray state/pause/quit/picker — команды | готово | [sync.md](tasks/sync.md) |
| SYNC-9a | 02.11.26 10:35 | PickerStarting → команда; доменные Message удалены, остался handshake | готово | [sync.md](tasks/sync.md) |
| SYNC-9b | 02.11.26 10:55 | Handshake/launcher на sync-кадры, Message/Incoming удалены | готово | [sync.md](tasks/sync.md) |
| SYNC-10 | 02.11.26 11:15 | Документация, замеры, финальный прогон | готово | [sync.md](tasks/sync.md) |
| ENC-1 | 02.11.26 12:10 | Память записи: EncoderTuning, zerolatency по умолчанию | готово | [encoder-memory.md](tasks/encoder-memory.md) |
| ENC-2 | 02.11.26 12:40 | GIF: two-pass палитра, память 2.1 GB → 1.07 GB на 4K | готово | [encoder-memory.md](tasks/encoder-memory.md) |
| RELAUNCH-1 | 02.11.26 13:05 | Повторный запуск открывает/фокусирует существующий UI | готово | [relaunch.md](tasks/relaunch.md) |
| ENC-3 | 02.11.26 13:30 | Настройки потоков/энкодеров для всех форматов — отложено, подробный документ | ожидание | [encoder-settings.md](tasks/encoder-settings.md) |
| SPLIT2-0 | 02.11.26 14:10 | Инвентаризация и ADR: один exe, роли, границы backend/frontend | готово | [backend-split.md](tasks/backend-split.md) |
| SPLIT2-1 | 02.11.26 14:25 | Protocol backend: library query/edit, preview, agent events | готово | [backend-split.md](tasks/backend-split.md) |
| SPLIT2-2 | 02.11.26 15:10 | Backend crate: Tokio runtime, library store, preview queue + cleanup | готово | [backend-split.md](tasks/backend-split.md) |
| SPLIT2-2b | 02.11.26 15:10 | Перенос daemon/sync/tray/hotkeys в backend | ожидание | [backend-split.md](tasks/backend-split.md) |
| SPLIT2-3a | 02.11.26 18:05 | Library snapshot; статические thumbnail UI через backend `preview_ensure` | в работе | [backend-split.md](tasks/backend-split.md) |
| SPLIT2-8a | 02.11.26 17:55 | AgentStream: Tokio broadcast + пример использования | готово | [backend-split.md](tasks/backend-split.md) |
| SPLIT2-P0 | 02.11.26 18:30 | Полный план и очередь SPLIT2 | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-B0 | 02.11.26 18:30 | Backend foundation: Tokio, library, preview, agent stream | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-F1 | 02.11.26 18:30 | Frontend foundation: крейт, Page, Notices, BackendClient | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-B2 | 02.11.26 18:30 | Previews backend: thumbnail/hover, очередь, TTL cleanup | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-B1 | 02.11.26 19:00 | Library DB — единственный источник; daemon на LibraryStore, без scan | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-F2 | 02.11.26 19:30 | Library frontend на LibraryView; edits в backend | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-F3 | 02.11.26 20:05 | Previews frontend: thumbnail/hover backend; production worker выключен | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-B3 | 02.11.26 20:35 | Recording backend: controller/host/plan/wire, daemon переключён | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-F4 | 02.11.26 21:05 | Recording frontend: types отдельно, controller/host test-only | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-B4 | 02.11.26 22:10 | Capture/screenshot/clipboard в backend; UI overlay держит кадр | готово (conditional) | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-F5 | 02.11.26 22:10 | Capture/picker frontend: оверлей и ввод | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-B5 | 02.11.26 23:10 | Conversion backend: controller + protocol start/cancel/state | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-F6 | 02.11.26 23:25 | Conversion frontend: старт в backend, status/progress по событиям | готово (conditional) | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-B6 | 02.11.26 22:45 | Settings/tray/hotkeys в backend daemon | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-F8 | 02.11.26 18:30 | Settings/tray/notices frontend | ожидание | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-B7 | 02.11.26 22:50 | AgentHost в backend; backend daemon его использует | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-B8 | 02.11.26 23:55 | Async agent host + live status/progress в backend | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-F7 | 03.11.26 00:40 | UI agent host view-only; backend всегда исполняет агента | готово (conditional) | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-F9 | 03.11.26 02:05 | Весь UI перенесён в mediaa-frontend; app — тонкий launcher | в работе | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-P1 | 02.11.26 22:45 | Один exe: --backend запускает mediaa_backend::daemon | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-P2 | 02.11.26 22:45 | Launcher запускает backend/UI; smoke двух процессов | готово | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-C1 | 02.11.26 18:30 | Удаление legacy и старых воркеров | ожидание | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-C2 | 02.11.26 18:30 | Полная тестовая матрица | ожидание | [backend-split-plan.md](tasks/backend-split-plan.md) |
| SPLIT2-C3 | 02.11.26 23:00 | Первый замер backend/UI процессов (debug) | в работе | [backend-split-measurements.md](tasks/backend-split-measurements.md) |
| FNL-1 | 03.11.26 02:20 | Старый app-код удалён; app — main+build icon | готово | [final-split.md](tasks/final-split.md) |
| FNL-2 | 03.11.26 02:40 | Frontend runtime без backend-daemon | готово | [final-split.md](tasks/final-split.md) |
| FNL-3 | 03.11.26 04:30 | Cache/thumb удалены; clipboard через backend protocol | готово | [final-split.md](tasks/final-split.md) |
| FNL-4 | 03.11.26 03:00 | Library view без storage в production | готово (conditional) | [final-split.md](tasks/final-split.md) |
| FNL-5 | 03.11.26 03:15 | Conversion production jobs отключены; только backend | готово (conditional) | [final-split.md](tasks/final-split.md) |
| FNL-6 | 03.11.26 03:45 | AgentView; tray runtime/storage/thumb/cache убраны из frontend | готово (conditional) | [final-split.md](tasks/final-split.md) |
| FNL-7 | 02.11.26 15:50 | P0 стабилизация split: app lib, app-agent, arch, clippy, baseline 386 UI-тестов | готово | [finalize-split-plan.md](tasks/finalize-split-plan.md) |
| FNL-8 | 02.11.26 17:10 | P1 backend-агент: core view-типы, host всегда в backend, rich snapshot, live events, op/task/steer/request, secrets load/merge, analyse/names/stop/title | готово (conditional) | [finalize-split-plan.md](tasks/finalize-split-plan.md) |
| FNL-9 | 02.11.26 17:10 | P2 frontend-агент: AgentView удалён, очередь/чат/models/providers/budget/catalogs/dialog на protocol; normal-дерево без backend/agent/engine/queue | готово | [finalize-split-plan.md](tasks/finalize-split-plan.md) |
| FNL-10 | 02.11.26 17:10 | P3 capture: protocol/backend/client, overlay и fullscreen/window через CaptureClient, frontend без mediaa-backend/storage/agent/engine/queue | готово | [finalize-split-plan.md](tasks/finalize-split-plan.md) |
| FNL-11 | 02.11.26 17:45 | P4/P5: conversion probe, library remove, Player preview, recording test-only; dev-зависимости пусты; открыт только rename файла и часть UI-тестов | готово (conditional) | [finalize-split-plan.md](tasks/finalize-split-plan.md) |
| FNL-12 | 02.11.26 17:35 | P6: release smoke двух процессов, exe 75.3 MiB, замеры 18.4/527.9 MB, architecture/plan/progress обновлены | готово (conditional) | [finalize-split-plan.md](tasks/finalize-split-plan.md) |
| FNL-13 | 02.11.26 17:45 | P4: library remove -> backend, Player preview -> backend, recording test-only, 5 view-тестов (334 passed) | готово (conditional) | [finalize-split-plan.md](tasks/finalize-split-plan.md) |
| FNL-14 | 02.11.26 17:55 | Rename файла -> backend (`LIBRARY_RENAME`), UI без fs::rename; conversion module allow убран; protocol roundtrip | готово (conditional) | [finalize-split-plan.md](tasks/finalize-split-plan.md) |
| ARC-1 | 02.11.26 20:35 | Frontend: `app/features/infra`; backend: `daemon/features/infra`, daemon разбит на dispatch/agent/services/notices/launch | готово | [crate-structure.md](tasks/crate-structure.md) |
| TST-1 | 02.11.26 21:00 | Восстановить 54 удалённых UI-теста агента (queue/analysis/task_details/dialog/chat) на wire-фикстурах; исходники приложены в `specs/tasks/removed-agent-ui-tests/` | ожидание | [restore-agent-ui-tests.md](tasks/restore-agent-ui-tests.md) |
| BUG-1 | 02.11.26 21:15 | Трей: подписи `backend message` заменены на локализованные; backend грузит locales и применяет язык настроек | готово | [tray-labels.md](tasks/tray-labels.md) |
| BUG-2 | 02.11.26 22:05 | Трей: ack+visible fallback для ShowWindow, быстрый Quit; pending events после Subscribe, без keychain; все `MEDIAA_DEV_TRAY` команды и `tray-action.ps1` | готово | [tray-open-quit.md](tasks/tray-open-quit.md) |

| ST-0 | 02.11.26 22:40 | Аудит всех состояний: master-план, 8 доменных задач, правила enum/event/effect и тестов без Windows | в работе | [state-machines.md](tasks/state-machines.md) |
| ST-1 | 02.11.26 23:07 | Tray/App lifecycle: protocol reducer + backend effects + WindowLifecycle; 24 targeted tests, smoke open/quit; 343 frontend, 53 backend | готово | [state-app-lifecycle.md](tasks/state-app-lifecycle.md) |
| ST-2 | 02.11.26 22:40 | Recording: одно состояние вместо RecState/ViewState/phase; cancel/discard не bool | ожидание | [state-recording.md](tasks/state-recording.md) |
| ST-3 | 02.11.26 22:40 | Capture/overlay: CaptureState вместо трёх Option<Job> и флагов restore/hide | ожидание | [state-capture.md](tasks/state-capture.md) |
| ST-4 | 02.11.26 22:40 | Conversion: ConversionState вместо job/batch_auto/probe_loading/conflict_notified | ожидание | [state-conversion.md](tasks/state-conversion.md) |
| ST-5 | 02.11.26 22:40 | Library/player: PlayerState вместо got_frame/audio_waiting/dragging | ожидание | [state-library.md](tasks/state-library.md) |
| ST-6 | 02.11.26 22:40 | Agent UI: reducer вместо глобального LiveBuffer и трёх Option; host state | ожидание | [state-agent.md](tasks/state-agent.md) |
| ST-7 | 02.11.26 22:40 | Backend services/sync: writer/clipboard/preview/sync state machines | ожидание | [state-services.md](tasks/state-services.md) |
| ST-8 | 02.11.26 22:40 | App shell: composition root вместо 45 полей и 858 строк | ожидание | [state-app-shell.md](tasks/state-app-shell.md) |

| ST-9 | 02.11.26 22:45 | Protocol/Sync: типизированные ClientCommand/ServerEvent/StoreId вместо строк и Vec<u8>; sync остаётся механизмом | ожидание | [state-protocol.md](tasks/state-protocol.md) |

| ST-10 | 02.11.26 22:50 | Ownership: protocol — чистые переходы, backend — авторитет/эффекты, frontend — проекция, sync — доставка | ожидание | [state-ownership.md](tasks/state-ownership.md) |

| ST-11 | 02.11.26 22:55 | Test strategy: pure transition, fake effects, protocol/sync, frontend projection; OS smoke отдельно | ожидание | [state-test-strategy.md](tasks/state-test-strategy.md) |

| ST-12 | 02.11.26 23:00 | Frontend-owned state: ClientPreferences/ui-state.toml, миграция viewer_volume из Settings, backend не знает | ожидание | [state-client-state.md](tasks/state-client-state.md) |

| RW-0 | 02.11.26 23:25 | Аудит и проект переписывания MediaA в `crates2`: 9 крейтов, роли, протокол, состояние, тесты, бюджет; агент исключён | готово | [rewrite-crates2.md](tasks/rewrite-crates2.md) |
| RW-1 | 02.11.26 23:55 | M0–M2: workspace `crates2`, core, i18n, protocol, storage, platform, media; 299 тестов | готово | [rewrite-crates2.md](tasks/rewrite-crates2.md) |
| RW-2 | 03.11.26 00:55 | M3–M4: backend IPC/library, frontend shell, страницы Library/Settings/Capture/Record/Audio/Convert | готово (conditional) | [rewrite-crates2.md](tasks/rewrite-crates2.md) |
| RW-3 | 03.11.26 01:07 | M5–M8: recording/conversion/screenshots, presence/tray/hotkeys, picker overlay; 382 passed / 1 ignored; реальные smoke: screenshot, MKV, MP3, MP4, cold launch | готово (conditional) | [rewrite-crates2.md](tasks/rewrite-crates2.md) |
| RW-4 | 03.11.26 01:18 | M7: picker с выделением, 8 ручками, Pen/Line/Arrow/Rect/Marker/Text, цветом, толщиной, undo/redo и выбором региона записи | готово (conditional) | [rewrite-crates2.md](tasks/rewrite-crates2.md) |
| RW-5 | 03.11.26 01:18 | M9 parity: видеопросмотр, hotkey capture, tray badges/язык, page-level kittest, чек-лист удаления старого | в работе | [rewrite-crates2.md](tasks/rewrite-crates2.md) |
