# MediaA 2: запуск и покрытие

## Запуск из корня проекта

```powershell
cd C:\shared-data\reps\.old\gifcap2
.\crates2\target\debug\mediaa.exe
```

Перед запуском закрой старый MediaA: у приложения один экземпляр на пользователя.

Роли:

```powershell
.\crates2\target\debug\mediaa.exe --tray   # только backend + трей
.\crates2\target\debug\mediaa.exe --ui     # только окно, если backend уже запущен
```

Красная dev-копия:

```powershell
.\crates2\target\debug\mediaa-agent.exe
```

## Изолированный запуск

Не трогает рабочие настройки и библиотеку:

```powershell
$root = Join-Path $PWD 'crates2\target\manual'
New-Item -ItemType Directory -Force $root,(Join-Path $root 'library'),(Join-Path $root 'data') | Out-Null
$env:MEDIAA2_INSTANCE_ID = 'manual'
$env:MEDIAA2_SETTINGS = Join-Path $root 'settings.toml'
$env:MEDIAA2_LIBRARY = Join-Path $root 'library'
$env:MEDIAA2_DATA_DIR = Join-Path $root 'data'
.\crates2\target\debug\mediaa.exe
```

Для красной dev-копии в изолированном окружении:

```powershell
$env:MEDIAA2_DEV_AGENT = '1'
.\crates2\target\debug\mediaa.exe
```

## Автоматические проверки

Полный workspace: **780 passed** (live-интеграция агента и реальный
захват рабочего стола — ignored, запускаются отдельно).

| Крейт | Тесты | Что покрыто |
|---|---:|---|
| agent-transcription | 252 (+12 ignored) | очередь (композиты, after-зависимости, чекпоинты/resume, scheduler), стадии pipeline/run, transcribe, диаризация, голоса, имена спикеров, индекс/поиск/темы, каталоги, веб (parse/readable), ассистент (brain, chat, tools, memory, title, step_title, quote), промпты (minijinja-снапшоты), host, transcript_db (file_id, per-media SQLite) |
| backend | 149 (+1 ignored) | settings, library query/rename/delete/cleanup, IPC handshake/state/quit, recording domains/plan/recovery, conversion queue/cancel, screenshots/picker, presence/tray/hotkeys/autostart, transcription-сервис (снапшот очереди, стриминг, транскрипт/темы/чат, провайдеры/модели/ключи, перенос имён по голосам) |
| core | 129 | settings, formats, geometry, query, recording, annotate, pacer, paths, naming, notices, navigation, frame, hash, agent-типы (connections, model roles, groups), speakers, chat-типы, pulse |
| frontend | 96 | все основные страницы в dark/light/narrow, модалки, меню Library, picker, viewer, convert statuses, markdown (layout/linkify/parse_time), mentions (roster/runs) |
| media | 64 | FFmpeg convert, GIF/WebP, filters, annotate, screenshot compose, preview cache, recorder |
| platform | 35 | tray menu/state/progress, hotkeys parsing, single instance, startup, screen, files, audio, ghost-окна |
| protocol | 15 | JSON roundtrip всех основных сообщений, wire framing/limits, транскрибационные команды/снапшоты (PROTOCOL=3) |
| model-eval | 29 (+1 ignored) | Engine trait, openai-парсеры (с локальным tiny_http), gate, slots, ollama (tags/pull), presets, keychain-секреты (memory store), ретраи (backoff/jitter/Retry-After/cancel), классификация ошибок |
| storage | 7 | schema, query/sort/pagination, corrupt rows, foreign schema |
| i18n | 3 | en/ru key parity, language resolution |

Плюс ручные end-to-end прогоны на изолированных каталогах:

- снимок экрана — PNG ~1.3 MB;
- 3-секундная запись видео — MKV ~174 KB;
- 3-секундная запись аудио — MP3 ~99 KB;
- конвертация MKV → MP4 — ~166 KB;
- холодный запуск лаунчера — backend + UI + library.db;
- второй UI — завершается, backend остаётся;
- транскрибация 11.5 мин русского аудио через локальный speech-сервер
  (GigaAM v3 + pyannote + CAM++): 1065 слов, диаризация, темы, чат по записи.

## Что посмотреть руками

1. **Трей**: иконка, меню, открытие окна, старт записи, quit.
2. **Screenshot**: Select area, выделение, ручки, Pen/Line/Arrow/Rect/Marker/Text, Copy/Save/Cancel.
3. **Record**: Screen/Windows/Region, нижний Start bar, status overlay, pause/stop/cancel, сохранение.
4. **Audio**: MP3/формат/битрейт, микрофон/системный звук, нижний Start bar, progress overlay.
5. **Convert**: несколько файлов, успех/ошибка/отмена, галочки/крестики, retry, Cancel.
6. **Library**: поиск/фильтр/сортировка, выделение, rename/delete, cleanup week/30 days, hover-видео, viewer, light theme.
7. **Транскрибация** (вкладки Ассистент/Провайдеры/Модели/Бюджет):
   - Провайдеры: добавить подключение (6 видов), имя/пул/URL/ключ/прокси,
     Check для Ollama, Start ollama serve, удаление;
   - Модели: роли (эмбеддинги/реранкинг/агент/названия/речь), выбор подключения
     и модели, Check models заполняет списки, ollama pull, контекст/thinking,
     диаризация/время слов/голосовые отпечатки для речи;
   - Бюджет: пулы с лимитами, слоты веб-поиска, движок поиска (DDG/SearXNG/Brave/Tavily),
     Test веб-поиска;
   - Ассистент: кнопка T у медиа-файла запускает анализ, очередь с прогрессом,
     T — транскрипт (спикер-чипсы, переименование), 📋 — темы, чат с markdown,
     стриминг ответа.
8. **Settings**: simple/advanced, выключение advanced с confirm, reset, autostart, hotkeys, папки, speech-сервер.
9. **Темы**: dark/light, светлый picker и viewer.
10. **Закрытие**: close-to-tray, повторный запуск, quit.

## Известные ограничения

- Viewer: встроенный плеер перенесён из старого приложения — кадры через ffmpeg-декодер
  в фоне, звук через AudioOut с синхронизацией, seek/скорость/громкость/пауза, анимации
  зациклены; «открыть в системном плеере» осталось вторичной кнопкой.
- Статусы Convert: Done берётся из `done`, Failed — из notice; отменённый файл показывается прочерком. Структурированного per-file результата в protocol пока нет.
- Picker-снепшоты используют синтетический кадр; реальный захват экрана тестами не запускается.
- Hotkey capture — на уровне egui-событий; конфликт проверяется нормализованными строками, не парсером слотов.
- В Windows 11 новая tray-иконка может попасть в overflow — её нужно один раз вытащить в панель.
- Транскрибация: RestartStage перезапускает задачу целиком (стадия игнорируется);
  поиск по индексу и восстановление чата (ChatOpen/ChatClose) ждут волны задач;
  ollama pull идёт в фоне, прогресс — в логе, список моделей обновляется по «Check models»;
  API-ключи хранятся в keychain и никогда не возвращаются в UI (поле пустое с подсказкой).
- Терминология приведена в порядок: крейт `mediaa2-agent-transcription`, вид
  подключения `speech_recognition` (старые настройки читаются через alias), команда
  запуска конвейера `Start` с опциями `TranscriptionOptions`, модуль стадий `stages`,
  PROTOCOL=3. Различение сохранено: транскрибация — процесс, транскрипт — документ.
- По сырому числу тестов новый workspace меньше старого: старый содержит ~1101 `#[test]`,
  включая legacy-снепшоты старого UI. Новые 770 сфокусированы на текущих путях
  и дополнены реальными e2e-прогонами.
- Волны задач (см. specs/progress.md): волна 1 (понятные ошибки модели, веб off по
  умолчанию, ретраи), волна 2 (модель закреплена за чатом с подтверждением, имена
  говорящих по голосовым отпечаткам, проверка модели перед скачиванием), волна 3
  (детект капчи DDG с советом, умное чтение страниц с фокусом) — готовы.
  Волна 3 по оригинальному todos.md: 5.1 — цепочка бесплатных движков DDG → Mojeek → Bing
  при капче; 5.3 — web_read(url, focus) режет страницу гибридно: слова (стемы) + эмбеддинги
  кусков той же моделью, что и записи; 2.2 — индексы эмбеддингов per-model (схема 7,
  таблица embeddings, ревизия транскрипта, старые векторы не удаляются — возврат модели
  возвращает индекс мгновенно; перераспознавание каскадом снимает все); UI индекса
  в транскрипте (готов/не построен/строится + кнопка); подсказки «похоже на X — принять?»
  для голосов средней уверенности (0.65–0.8); кеш страницы на время ответа агента.
  Неприменимо к crates2: 1.1 (форматы sherpa-onnx — локального движка больше нет,
  распознавание через speech-сервер). Evals (9) — отдельный крейт и эталонные записи,
  предлагается отдельным решением; 8.1 (промпты) — процессная задача под бенч/evals.
