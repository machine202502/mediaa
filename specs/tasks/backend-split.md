# SPLIT2: backend и frontend в одном бинарнике

Полный план и очередь: [backend-split-plan.md](backend-split-plan.md).

Статус: SPLIT2-0…2 готовы; SPLIT2-3a в работе, SPLIT2-2b ждёт. Цель — один `mediaa.exe`, который запускает две роли-процесса,
но собран из раздельных крейтов:

- `mediaa-backend` — вся логика: БД, запись, capture, превью, агент, tray/hotkeys,
  sync-server, Tokio.
- `mediaa-frontend` — только отрисовка: egui, страницы, пикер, модалки; читает
  файлы напрямую, не открывает БД.
- `mediaa-agent` — отдельный крейт, зависимость backend.
- `crates/app` — тонкая сборка одного бинарника и выбор роли.

## Решения от 02.11.26

- Один exe, две роли-процесса: launcher / `--backend` / `--ui`.
- Frontend читает файлы с диска (библиотека, видео, скриншоты, файлы агента,
  миниатюры), но не пишет и не открывает SQLite. Байты файлов по IPC не идут.
- Захват экрана, запись, кодирование, превью, миниатюры, очистка кэша — backend.
- Библиотека — это `library.db`; путь остаётся текущим:
  `media_dir/.cache/library.db`.
- Периодического сканирования папки и пользовательского импорта нет.
- В библиотеку попадает только созданное приложением: скриншот, запись
  видео/аудио; конвертация индексируется только если выходная папка — папка
  библиотеки.
- Миграций и обратной совместимости нет; схему и настройки можно менять.
- Превью — ленивые: UI просит, backend отдаёт готовый путь или ставит в очередь.
  Один генератор, статусы `Ready/Pending/Failed`, без heartbeat.
- Очистка превью — по таблице SQLite, периодическая задача backend.
- Агент всегда в backend; отдельного процесса/AgentHost в UI нет.
- Сначала весь backend переводится на Tokio, затем агент полностью на async;
  live-стрим агента делается сразу, с примером в проекте.

## Целевая структура

```text
mediaa.exe
  role launcher  -> spawn current_exe --backend / --ui
  role backend   -> mediaa_backend::run()
  role ui        -> mediaa_frontend::run(args)
```

Зависимости:

```text
app -> backend + frontend
backend -> core, storage, media, platform, protocol, sync, agent, queue
frontend -> core, protocol, sync, render? (только для отрисовки)
```

Frontend не должен зависеть от `rusqlite`, `mediaa-agent`, `mediaa-storage`,
`mediaa-backend` и FFmpeg-энкодеров. Исключение на переходный период: чтение
медиафайлов и декодирование для просмотра, если это остаётся в UI.

## Границы ответственности

| Область | Backend | Frontend |
|---|---|---|
| SQLite `library.db` | владеет, читает/пишет | не открывает |
| Список файлов | query/list/version | рисует view-model |
| Запись/capture/скриншоты | да | нет |
| Кодирование/конвертация | да | нет |
| Миниатюры/превью | генерация, кэш, очистка | запрашивает путь, читает файл |
| Удаление/rename/edits | да | только команда |
| Настройки | writer + store | отправляет команды |
| Tray/hotkeys/notices | да | рисует уведомления |
| Агент | host, очередь, live stream | рисует события |
| Окно/пикер/оверлей | нет | да |
| Чтение медиа | может | да, read-only |

## Библиотека

Схема (черновик, без миграций):

```sql
files(id, path, kind, title, created_ms, modified_ms, size, width, height,
      duration, audio, details_json)
previews(file_id, kind, path, state, requested_at, last_requested_at, error)
preview_jobs(id, file_id, kind, state, created_at, started_at, finished_at)
```

- `library.db` — единственный источник списка; файловая система не сканируется.
- Запись/скриншот/аудио создают строку сразу после успешного сохранения.
- Конвертация: если output внутри `media_dir` — insert/update; иначе только файл.
- Удаление файла — backend удаляет строку и связанные превью.
- UI получает список через protocol; пагинация/фильтры на backend.

## Превью

UI:

```text
preview.ensure(file_id, kind) -> Ready { path } | Pending | Failed
```

Backend:

- если строка `previews` в состоянии `ready` и файл существует → `Ready(path)`;
- если `generating`/`queued` → `Pending`;
- иначе ставит job, `Pending`; один worker;
- завершение — событие `preview.changed(file_id, state, path, error)`.

Очистка:

- периодически читает таблицу `previews`, а не папку;
- удаляет файл и строку, если `last_requested_at` старше TTL;
- `generating` не удаляет;
- константы в коде:

```text
PREVIEW_WORKERS = 1
PREVIEW_TTL = 7 days
PREVIEW_CLEANUP_INTERVAL = 15 min
PREVIEW_JOB_TIMEOUT = 2 min
```

UI читает готовый файл напрямую; bytes по IPC не передаются.

## Агент и Tokio

Этап 1:

- backend роль создаёт Tokio multi-thread runtime;
- SQLite/FFmpeg/файлы — `spawn_blocking`;
- текущий `mediaa-agent::run::spawn_host` запускается как blocking task
  (`spawn_blocking`), без отдельного процесса;
- UI не инициализирует Tokio.

Этап 2:

- убрать `spawn_host` std-thread и внутренние `std::thread` агента;
- заменить блокирующие `ureq`, `sleep`, файловые операции на async/`spawn_blocking`;
- очередь и live-события — async.
- `mediaa-agent` остаётся отдельным крейтом.

Live stream:

- protocol-события: текст/чанк, статус, tool, прогресс, завершение;
- backend публикует в sync EventLog;
- UI подписывается и рисует;
- пример использования стрима — отдельный example/test в `crates/agent` или
  `crates/backend`.

## Задачи

### SPLIT2-0. Инвентаризация и ADR
- карта модулей `app` (43k строк): что переезжает, что остаётся;
- карта зависимостей `logic`/`adapters`/`shell`/`ui`;
- baseline RSS/CPU/старт;
- зафиксировать границы и layer-gates.

**Готово:** этот документ + карта; baseline.

### SPLIT2-1. Protocol
Добавить в `mediaa-protocol`:
- `library.query`, `library.changed`, `library.edit`;
- `preview.ensure`, `preview.changed`;
- `agent.event`, `agent.state`;
- DTO без SQLite/egui;
- версия протокола и сериализация.

**Готово:** roundtrip-тесты, контракт используется backend/frontend.

### SPLIT2-2. Backend crate
- создать `crates/backend` (`mediaa-backend`);
- Tokio runtime;
- library store (`library.db`, query/edit/delete);
- preview queue (один воркер, lazy, `Ready/Pending/Failed`, TTL-константы);
- перенести daemon, sync-server, settings writer, tray/hotkeys, notices, recovery;
- запуск из `mediaa.exe --backend`.

**Сделано:** runtime, library store, preview worker и тесты.

**Готово:** backend не зависит от egui/frontend.

### SPLIT2-2b. Перенос daemon
- sync-server, settings writer, tray/hotkeys, notices, recovery, запись;
- роль `mediaa.exe --backend`;
- backend владеет фоновыми задачами.

**Готово:** UI подключается к новому backend; старый daemon удалён.

### SPLIT2-3a. UI view-models (в работе)
Логика UI сейчас использует inherent-impl на типах из `logic` (`Page`, `Notices`,
`ConversionController`, `LibraryController`, `Viewer`) и тестовые методы. При переносе
`logic` в backend это невозможно из-за orphan rules. Сначала выделить эти UI-типы/методы
в frontend (или перевести на локальные extension traits), затем переносить логику.

**Сделано:** создан `crates/frontend`; `Page`, `Notices`, `BackendClient` добавлены;
в app остались `PageUi`/`NoticesUi`-traits.
`BackendClient` формирует `library_query`, `preview_ensure` и sync-команды, не зная UI.
Daemon уже отвечает на `library_query` (`LibraryPageWire`) из своего `LibraryIndex`.
Дополнительно daemon публикует полный `LibraryPageWire` в `library` store; UI применяет его
через `LibraryIndex::from_entries` — в обычном flow UI больше не открывает БД и не сканирует
папку. Legacy `reload` оставлен только для тестов.
`preview_ensure` подключён к `mediaa_backend::preview::PreviewHandle` (Tokio runtime,
`PreviewStore` в той же `library.db`, отдельная таблица `backend_previews`); ready/failed
события уходят в sync как `preview`.
**Осталось:** контроллеры и `Viewer`. Они держат `LibraryIndex`/медиа-логику, поэтому
переносятся после backend-API списка и превью; иначе frontend снова начнёт читать БД.
`Viewer`/`Player` — кандидаты на frontend, так как это read-only воспроизведение.
`LibraryController` уже получает snapshot из backend. Статические thumbnail в UI теперь
запрашиваются через `preview_ensure`; backend отдаёт путь, UI читает файл и создаёт текстуру.
Остались hover/animated previews, которые UI всё ещё генерирует сам.
Тестовые helper-методы переезжают вместе с типами.

**Готово:** UI-типы не определяют inherent-impl на backend-типах; тесты UI остаются в
frontend.

### SPLIT2-3. Frontend crate
- создать `crates/frontend` (`mediaa-frontend`);
- перенести shell/ui/страницы/пикер;
- sync-client, чтение файлов;
- убрать `rusqlite`, `mediaa-agent`, storage/encoding зависимости.

**Готово:** `cargo tree -p mediaa-frontend` без запрещённых зависимостей;
layer-check.

### SPLIT2-4. Library DB
- backend открывает `media_dir/.cache/library.db`;
- удалить `scan_dir`/rescan/UI-индекс;
- запись/скриншот/аудио добавляют строки;
- конвертация индексирует только output в `media_dir`;
- удаление/rename обновляют БД.

**Готово:** UI не открывает БД; нет сканирования; тесты.

### SPLIT2-5. Library query/UI
- backend query с фильтрами/сортировкой/пагинацией;
- UI view-model вместо `LibraryIndex`;
- обновления через версии/события.

**Готово:** список совпадает с БД; UI не знает путь БД.

### SPLIT2-6. Previews
- очередь, один worker, статусы, события;
- TTL-очистка по таблице;
- UI запрашивает и читает файл.

**Сделано в backend:** lazy request, один worker, thumbnail/hover generation,
`Ready/Pending/Failed`, TTL + периодическая cleanup-задача, тесты.
**Осталось:** protocol-интеграция с daemon/UI.

**Готово:** ready/pending/failed; нет параллельной генерации; кэш чистится.

### SPLIT2-7. Edit commands
- rename/delete/метаданные только backend;
- UI не вызывает fs-операции библиотеки;
- ошибки возвращаются.

### SPLIT2-8. Agent always backend
- убрать `AgentHost` из UI и handoff;
- backend владеет очередью;
- live stream события;
- нет отдельного процесса.

**Сделано:** `AgentStream` (Tokio broadcast) + пример `crates/backend/examples/agent_stream.rs`.
**Осталось:** перенести реальный `AgentHost`, сериализовать live-события, убрать UI host.

### SPLIT2-9. Agent async
- убрать `spawn_host` thread;
- блокирующие вызовы заменить/обернуть;
- Tokio tasks, cancellation, graceful shutdown.

### SPLIT2-10. One binary/roles
- `current_exe --backend/--ui`;
- launcher, relaunch-to-focus;
- один exe в упаковке/скриптах;
- Task Manager: два процесса с одним именем.

### SPLIT2-11. Cleanup/tests/measure
- удалить старые модули/зависимости;
- layer gates, protocol tests, integration;
- RSS/CPU/startup;
- docs/progress.

## Порядок

SPLIT2-0 → 1 → 2/3 → 4/5 → 6 → 7 → 8/9 → 10/11.

## Риски

- UI сейчас глубоко использует `logic`; вынос потребует временных адаптеров и
  публичных DTO, иначе большой одномоментный рефакторинг.
- `mediaa-storage::LibraryIndex` сейчас смешивает скан, БД и кэш; нужно разрезать
  на backend store и frontend view-model.
- Агент использует std-thread и блокирующие `ureq`; async-переход нельзя делать
  одним `tokio::spawn`.
- Один exe = оба процесса с одним именем; отказаться от разных имён процессов.
- Память UI определяется egui/wgpu; разделение крейтов само по себе её не снизит,
  но позволит заменить frontend позже.
- Не ломать запись/агент во время миграции; каждый этап должен иметь зелёные тесты.

## Открытые вопросы

- Оставляем ли декодирование видео для просмотра во frontend (сейчас склоняемся
  «да, read-only»).
- Нужен ли отдельный `agent.state` snapshot или достаточно событий.
- Нужны ли previews для файлов агента в той же таблице `previews` или отдельный
  namespace.
- Точные поля `details_json` и edit-команд.
