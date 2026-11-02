# SPLIT2: полный план и очередь

Статус: план зафиксирован 02.11.26. Это единственный источник порядка работ.
Старый [backend-split.md](backend-split.md) — исторический контекст; при расхождении
действует этот документ.

## 1. Цель

Один `mediaa.exe`, две роли-процесса:

```text
mediaa.exe              launcher
mediaa.exe --backend    headless backend: Tokio, БД, запись, capture, превью,
                        настройки, tray/hotkeys, agent host
mediaa.exe --ui         frontend: только рисование, ввод, чтение файлов
```

Крейты:

```text
crates/backend   mediaa-backend
crates/frontend  mediaa-frontend
crates/agent     mediaa-agent (отдельный, без изменений)
crates/app       тонкая сборка одного exe и выбор роли
crates/protocol  весь контракт
crates/sync      транспорт/движок синхронизации
```

Жёсткие правила:

- backend — единственный писатель `library.db`;
- frontend не открывает `library.db`, не сканирует папку, не создаёт превью;
- frontend читает медиа/превью/файлы агента напрямую с диска;
- файлы по IPC не передаются, только пути/состояния/команды;
- backend генерирует thumbnail/hover/animated previews;
- backend очищает кэш по таблице SQLite и TTL;
- агент всегда живёт в backend; отдельного процесса/AgentHost в UI нет;
- один exe; два процесса, один образ.

## 2. Что уже есть

| Кусок | Состояние |
|---|---|
| `mediaa-sync` (кадры, версии, команды, события, resync) | готово |
| `mediaa-protocol`: settings/recording/library/notices/recovery/tray/hotkeys | готово |
| `mediaa-protocol::backend`: library query/edit, preview, agent events/state | готово |
| `mediaa-backend`: Tokio runtime, library store, preview queue + TTL | готово |
| `mediaa-backend::agent_stream` + пример | готово |
| daemon `library_query`, `preview_ensure`, snapshot библиотеки | готово |
| `mediaa-frontend`: `Page`, `Notices`, `BackendClient` | готово |
| UI: библиотека через snapshot, статические thumbnail через backend | частично |
| UI: hover/animated previews | пока UI генерирует сам |
| UI: контроллеры library/conversion/recording, Viewer | пока в app |
| UI: AgentHost/handoff | пока в app |
| daemon/sync/settings/tray/hotkeys в app | пока в app |

## 3. Границы по модулям

Строки — текущие файлы `crates/app/src`; линии приблизительны.

| Область | Строк | Судьба | Задача |
|---|---:|---|---|
| `ui/pages/**` | 22 599 | frontend | F2–F8 |
| `ui/region_editor*` | 1 994 | frontend overlay | F5 |
| `ui/notices.rs`, themes/widgets | 1 900 | frontend | F1 done/partial |
| `shell/**` | 6 210 | frontend | F2–F8 |
| `logic/library/**` | 2 024 | split: view/controller → frontend; DB/index → backend | B1, F2, F3 |
| `logic/recording/**` | 1 278 | backend; view model → frontend | B3, F4 |
| `logic/conversion/**` | 1 071 | backend; view/controller → frontend | B5, F6 |
| `logic/capture.rs`, `screenshot.rs` | 229 | backend capture; overlay → frontend | B4, F5 |
| `logic/agent_host.rs` | 167 | backend | B7, B8 |
| `logic/hotkeys.rs` | 214 | backend | B6 |
| `logic/region_editor.rs` | 149 | frontend overlay | F5 |
| `logic/jobs.rs`, `latest_worker.rs` | 188 | shared utility/core | B0 |
| `logic/intent.rs`, `command.rs`, `navigation.rs`, `window_presence.rs` | 130 | frontend | F1–F5 |
| `logic/cache_maintenance.rs` | 59 | backend | B2 |
| `logic/notices.rs` | data → core/frontend done | — | F1 |
| `daemon.rs` | 1 772 | backend | B0/B3/B6 |
| `adapters/ipc.rs` | 148 | shared transport | B0 |
| `adapters/settings_file.rs` | 203 | backend | B6 |
| `adapters/tray.rs`, `hotkeys.rs` | 302 | backend | B6 |
| `adapters/clipboard*` | 316 | backend | B6 |
| `adapters/native_window`, `desktop`, `dialogs`, `files` | 70 | frontend | F5/F8 |
| `wire.rs` | 273 | protocol/shared | B0 |
| `icon.rs`, `i18n.rs`, `dev_mode.rs` | 542 | frontend | F1 |
| `roles.rs`, `entry.rs`, `main.rs` | 94 | app binary | P1/P2 |

## 4. Задачи

Формат: ID, цель, границы, зависимости, критерий готовности.

### SPLIT2-P0. Инвентаризация и очередь
- этот документ + карта модулей;
- очередь ниже зафиксирована.
**Готово:** документ принят, progress обновлён.

### SPLIT2-B0. Backend foundation
**Дополнено:** `Job` и `LatestWorker` вынесены в `mediaa-core`; `Page` — в
`mediaa_core::navigation` (backend и frontend используют один тип).
- `crates/backend`, Tokio runtime;
- `LibraryStore`, `PreviewStore`, preview queue + cleanup;
- `AgentStream`;
- arch-правила.
**Готово:** тесты backend, крейт без egui.

### SPLIT2-F1. Frontend foundation
- `crates/frontend`;
- `Page`, `Notices`, `BackendClient`;
- extension traits в app для UI-методов.
**Готово:** frontend-крейт, тесты, arch.

### SPLIT2-B1. Library DB — единственный источник
**Готово:** daemon открывает `LibraryStore` в `media_dir/.cache/library.db`; startup-скан
убран; запись/скриншот/конвертация добавляют пути командами; UI-БД в обычном flow не
открывается. Rename/delete-команды и полная F2-интеграция — в F2.
- свести `LibraryStore` и текущий `LibraryIndex` к одной схеме
  `media_dir/.cache/library.db`;
- backend — единственный писатель;
- добавить/обновить строки: recording/screenshot/audio, conversion только в media_dir;
- удалить UI-запись `set_details/flush_details`;
- snapshot/query из backend.
**Зависит:** B0.
**Готово:** backend-тесты схемы, UI не пишет в БД, snapshot содержит все поля.

### SPLIT2-B2. Previews backend
- thumbnail/hover/animated preview jobs;
- один worker, `Ready/Pending/Failed`, события;
- TTL cleanup по таблице.
**Сейчас:** thumbnail/hover + cleanup готовы; animated нужно добавить.
**Готово:** UI может получить любой preview без локальной генерации.

### SPLIT2-B3. Recording backend
- перенести `RecordingController`/`host` в backend;
- протокол start/stop/pause/cancel уже есть;
- состояния/события — через sync;
- capture loop остаётся media.
**Зависит:** B0, A2 (контракт уже есть).
**Готово:** backend пишет, UI только командует/рисует.

### SPLIT2-B4. Capture/screenshot backend
- `capture.rs`, `screenshot.rs`, region/window capture;
- backend отдаёт результат/путь, UI — оверлей и picker;
- clipboard — backend.
**Зависит:** B0, A2.
**Готово:** UI не вызывает capture/encoding напрямую.

### SPLIT2-B5. Conversion backend
- conversion jobs/controller: probe/start/cancel/progress;
- команды/события protocol;
- UI только форма/прогресс.
**Зависит:** B0, A2.
**Готово:** conversion не зависит от egui.

### SPLIT2-B6. Settings/tray/hotkeys/notices backend
- settings writer и sync-store;
- tray/hotkeys/уведомления;
- backend владеет жизненным циклом.
**Зависит:** B0.
**Готово:** UI не владеет tray/hotkeys, только команды.

### SPLIT2-B7. AgentHost backend
- перенести `AgentHost` в backend;
- UI больше не создаёт host;
- сериализуемые `AgentState`/`AgentEvent`.
**Зависит:** B0.
**Готово:** агент работает без UI-хоста.

### SPLIT2-B8. Agent async
- убрать `spawn_host` std-thread;
- blocking I/O → async/`spawn_blocking`;
- live stream через `AgentStream`.
**Зависит:** B7.
**Готово:** агент — Tokio-задача, UI получает live.

### SPLIT2-F2. Library frontend
**Готово:** `LibraryController` на `LibraryView`; данные из snapshot; query/filter/sort —
чистый view-model по snapshot (backend `library_query` остаётся для paging); rename/delete —
команды backend; production load/scan/write удалены. Test-only scan остаётся до F3/C1.
**Зависит:** B1.
**Готово:** UI не импортирует storage и не открывает БД.

### SPLIT2-F3. Previews frontend
**Готово:** static thumbnail и hover-proxy генерирует backend; `preview_worker` удалён;
production `thumb_worker` выключен. UI декодирует animated proxy как read-only playback;
тестовый `thumb_worker` удаляется в C1.
**Зависит:** B2, F2.

### SPLIT2-F4. Recording frontend
- view/controller без backend-логики;
- команды start/stop/pause/cancel;
- события/состояния из sync.
**Зависит:** B3.
**Готово:** UI не владеет recorder.

### SPLIT2-F5. Capture/picker frontend
- оверлей, region editor, picker;
- capture requests — backend;
- окно/ввод — UI.
**Зависит:** B4.
**Готово:** UI не захватывает/не кодирует.

### SPLIT2-F6. Conversion frontend
- форма, очередь, прогресс;
- все операции — команды backend.
**Зависит:** B5.
**Готово:** UI не запускает media jobs.

### SPLIT2-F7. Agent frontend
- убрать `AgentHost`/handoff;
- рисовать `AgentState`/`AgentEvent`, live stream.
**Зависит:** B7/B8.
**Готово:** UI не владеет очередью агента.

### SPLIT2-F8. Settings/tray/notices frontend
- только UI и команды;
- статусы tray/hotkeys из sync.
**Зависит:** B6.
**Готово:** UI не регистрирует hotkeys, не владеет tray.

### SPLIT2-F9. Frontend dependency cleanup
**Сделано:** весь UI/shell/view-контроллеры перенесены в `mediaa-frontend`; `crates/app` — только `main` + launcher.
**Остаток:** frontend временно зависит от backend-контроллеров; agent UI использует `AgentServices`.
Нужны отдельные подшаги:
- F9a: send через `agent_send` (backend API готов);
- F9b: stop/resume через `AgentTaskWire` (API готов);
- F9c: live timeline из `agent`/`agent_state`;
- F9d: чтение чатов/каталогов только с диска;
- F9e: удалить `AgentHost` из `Ctx`/App;
- F9f: `cargo tree` без storage/agent/media-энкодеров.
- `cargo tree -p mediaa-frontend` без storage/agent/media-энкодеров;
- если playback остаётся — только read-only media decode, отдельно зафиксировать.
**Зависит:** F2–F8.

### SPLIT2-P1. Один exe, роль backend
- `mediaa.exe --backend` запускает `mediaa_backend::run`;
- `--ui` — frontend;
- launcher — current exe;
- один файл в упаковке.
**Зависит:** B2–B8, F9.

### SPLIT2-P2. Lifecycle/relaunch
- повторный запуск открывает UI;
- backend стартует/останавливается по lifecycle;
- tray/quit/ heartbeat.
**Зависит:** P1.

### SPLIT2-C1. Удаление legacy
- удалить старый daemon/логику из app;
- удалить `thumb_worker`/`preview_worker`/`LibraryIndex` из UI;
- удалить неиспользуемые зависимости.
**Зависит:** F9, P1.

### SPLIT2-C2. Тесты
- контракт library/preview/recording/capture/conversion/agent;
- обрыв/переподключение/потеря/дубли/порядок;
- один preview worker;
- agent stream;
- UI без БД/сети.
**Зависит:** C1.

### SPLIT2-C3. Замеры и документация
- RSS/CPU/startup до/после;
- архитектура, protocol, deferred;
- обновить progress/AGENTS при необходимости.
**Зависит:** C2.

## 5. Очередь

Строки `SPLIT2-*` в `progress.md`, созданные до `SPLIT2-P0`, — исторический контекст.
Авторитетна только таблица ниже и строки `SPLIT2-P0…C3` в `progress.md`.

Работать строго сверху вниз; следующую задачу начинать только после критерия готовности
предыдущей. Если задача требует временного адаптера — он удаляется в C1.

| # | ID | Статус | Зависит |
|---:|---|---|---|
| 1 | SPLIT2-P0 | готово | — |
| 2 | SPLIT2-B0 | готово | — |
| 3 | SPLIT2-F1 | готово | — |
| 4 | SPLIT2-B2 | готово (кроме animated) | B0 |
| 5 | SPLIT2-B1 | готово | B0 |
| 6 | SPLIT2-F2 | готово | B1 |
| 7 | SPLIT2-F3 | готово | B2, F2 |
| 8 | SPLIT2-B3 | готово | B0 |
| 9 | SPLIT2-F4 | готово | B3 |
| 10 | SPLIT2-B4 | готово (conditional) | B0 |
| 11 | SPLIT2-F5 | готово | B4 |
| 12 | SPLIT2-B5 | готово | B0 |
| 13 | SPLIT2-F6 | готово (conditional) | B5 |
| 14 | SPLIT2-B6 | готово | B0 |
| 15 | SPLIT2-F8 | ожидание | B6 |
| 16 | SPLIT2-B7 | готово | B0 |
| 17 | SPLIT2-B8 | готово | B7 |
| 18 | SPLIT2-F7 | готово (conditional) | B7/B8 |
| 19 | SPLIT2-F9 | следующая | F2–F8 |
| 20 | SPLIT2-P1 | готово | B2–B8, F9 |
| 21 | SPLIT2-P2 | готово | P1 |
| 22 | SPLIT2-C1 | ожидание | F9, P1 |
| 23 | SPLIT2-C2 | ожидание | C1 |
| 24 | SPLIT2-C3 | в работе | C2 |

## 6. Правила выполнения

1. Перед началом задачи: статус `в работе` в `progress.md`.
2. Не начинать следующую, пока текущая не закрыта тестами.
3. Каждая задача заканчивается:
   - `cargo test` затронутых крейтов;
   - clippy;
   - обновлением этого документа (факты/риски);
   - строкой в `progress.md`.
4. Нельзя добавлять UI-настройки/пресеты вне задач; параметры только в коде до F8.
5. Если задача требует временного дублирования кода — явно отметить в разделе задачи и
   удалить в C1.
6. Никаких миграций БД; схема может быть пересоздана.
7. Один exe: не добавлять `mediaa-backend.exe`/`mediaa-ui.exe`.

## 7. Фактический статус на 03.11.26 09:00

Готово: `P0`, `B0`, `B1`, `F1`, `F2`, `F3`, `B3`, `F4`, `B4` (conditional), `F5`, `B6`, `B7`, `P1`, `P2`.
`--backend` — отдельный процесс `mediaa_backend::daemon`; `--ui` — текущий egui app; launcher запускает оба. Smoke подтверждён.
Осталось: `B5`/`F6` (conversion UI всё ещё app), `B8`/`F7` (async agent + UI без AgentHost), `F8` (UI settings/tray cleanup), `F9` (frontend deps), `C1–C3`.
При текущем темпе они не помещаются в оставшиеся 5 раундов; нужен либо новый лимит, либо сокращение объёма.
B4 нельзя закрыть без переноса image-clipboard: кадр сейчас живёт в UI, а байты по IPC не передаются.

## 7. Риски

- `LibraryController`/`ConversionController`/`Viewer` смешивают UI и media/storage;
  F2/F3/F6 — самые большие задачи.
- `AgentHost` использует Scheduler/Streams/Feed в памяти; B7 требует сериализации
  live-состояния (частично сделано AgentStream).
- Полный snapshot библиотеки в одном кадре может упереться в `MAX_FRAME_BYTES` на
  больших библиотеках; при необходимости — paging/invalidations.
- UI playback: если decode остаётся во frontend, это надо явно зафиксировать в F9;
  иначе — отдельный backend media stream.
- Один exe = два процесса с одним именем; разные имена процессов больше не цель.
