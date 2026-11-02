# FINALIZE SPLIT — задача для новой сессии

Статус на момент передачи: **не завершено**. Приложение собирается, процессы разделены,
но frontend ещё зависит от `backend/agent/engine/queue`. Нельзя выдавать это за «готово».

## 1. Цель

```text
crates/app       только main + roles + build icon
crates/frontend  весь UI, view-модели, transport, чтение файлов
crates/backend   вся логика: БД, запись, capture, previews, conversion,
                 settings/tray/hotkeys, agent host/stream
```

Один `mediaa.exe`, роли `launcher / --backend / --ui`. Backend — отдельный процесс.
Frontend не зависит от `backend`, `storage`, `agent`, `engine`, `queue`.
Файлы (медиа, превью, capture, чаты) frontend читает напрямую; байты по IPC не передаются.

## 2. Зафиксированные решения

- финальный скриншот компонует **frontend** через `mediaa_render` в native resolution;
- временный RGBA — только по backend-issued пути/id;
- форматы — **только PNG и WebP** (в `mediaa_core::ImageFormat` нет JPEG);
- agent UI — **путь A**: никакого прямого доступа к `Scheduler/Store/Streams/Feed/Engine`;
- лимиты памяти не ставим;
- БД можно сбрасывать, миграции не делать;
- один exe, два процесса; AgentHost всегда backend, UI не запускает host.

## 3. Что уже сделано (не переделывать)

- `app`: `main.rs` → `mediaa_frontend::entry::run()`; `entry/roles` в app; `icon.rs` нужен build.rs.
- `frontend` содержит `ui/`, `shell/`, view-логику; `AgentView` вместо `AgentHost` (host не тикает).
- `backend` содержит daemon, library store, recording, conversion, capture, previews, clipboard,
  settings/tray/hotkeys, AgentHost, async host, agent stream.
- Protocol:
  - library query/edit/add, previews;
  - recording;
  - conversion start/cancel/state;
  - agent send/send_result/stop/resume/state/event/snapshot;
  - clipboard_copy;
  - capture_request / screenshot_commit.
- backend capture-to-file:
  - `capture_request { monitor_index }` → `CaptureSourceWire { path,width,height,scale }`;
  - `.cache/capture/*.rgba`, header `MCA1` + width/height/stride + pixels;
  - `screenshot_commit { source, output, format, copy_image, copy_file }`:
    читает RGBA, кодирует PNG/WebP, сохраняет, копирует, удаляет source.
- frontend clipboard proxy отправляет `clipboard_copy`; backend владеет ClipboardService.
- `AgentSnapshotWire`/`AgentTaskViewWire` публикуется backend’ом; frontend `live_buffer` его принимает.
- Удалены: production thumb_worker, cache_maintenance/cache-sweep, tray runtime, storage из normal deps.
- app build icon exception: `crates/app/src/icon.rs` — только build asset.

## 4. Проблема 1: Agent UI

### Почему блокер

`QueueUi`, `CatalogsUi`, `Chat`, `Studio`, `ModelsUi` используют `mediaa_queue::Task`, `Scheduler`,
`Store`, `Streams`, `mediaa_engine` напрямую. Backend snapshot уже есть, но UI не переведён.
Без этого нельзя убрать `agent/engine/queue` из frontend.

### Задачи

#### A1. Queue на snapshot
Файлы:
- `crates/frontend/src/ui/pages/agent/queue.rs`
- `crates/frontend/src/ui/pages/agent/live_buffer.rs`
- `crates/frontend/src/ui/pages/agent/catalogs.rs`

Сделать:
- `QueueUi` хранит `AgentSnapshotWire`/собственный `QueueView`, а не `queue::Snapshot`;
- маппинг `AgentTaskViewWire` → строка/статус/прогресс;
- фильтры/группировка по `kind/scope/status` остаются UI-логикой;
- действия pause/resume/cancel/restart/remove — команды protocol (сейчас есть stop/resume; добавить
  `agent_pause`, `agent_restart`, `agent_remove` или generic `agent_op`);
- ошибки — notices.

Acceptance:
- `QueueUi` не импортирует `mediaa_queue`;
- очередь отображается по snapshot;
- действия работают через protocol;
- UI тесты очереди зелёные.

#### A2. Чаты/каталоги — файлы
Файлы:
- `ui/pages/agent/chat.rs`
- `ui/pages/agent/catalogs.rs`
- `ui/pages/agent/studio.rs`
- `ui/pages/agent/topics.rs`
- `ui/pages/agent/task_details.rs`

Сделать:
- читать транскрипты/каталоги через `mediaa_core::transcript`/файловые API;
- убрать `services.sched.store()`, `services.streams`, `services.feed`, `services.engine`;
- отправка сообщений — `agent_send` (уже есть);
- stop/resume — protocol;
- title/steer — protocol-команды или файлы;
- модели/провайдеры — backend commands (список/тест соединения), не `Engine`.

Acceptance:
- чат открывается без `AgentServices`;
- новые сообщения уходят в backend;
- ответы/live читаются из файлов/events;
- каталоги/история работают.

#### A3. Live timeline
- `AgentEventWire` уже содержит `task`.
- Отображать `Text/Status/Tool/Progress/Done/Error` из событий.
- `live_buffer` — единый источник live.
- `Live.timeline` не использовать.

#### A4. Удалить AgentView/AgentServices
Файлы:
- `frontend/src/logic/agent_host.rs`
- `frontend/src/shell.rs`
- `frontend/src/ui/pages/ctx.rs`
- `frontend/src/shell/test_support.rs`

Удалить поле `agent_host`/`AgentView`, импорты `mediaa_agent`, `mediaa_engine`, `mediaa_queue`.
Заменить на `AgentClient` (protocol) + file readers.

## 5. Проблема 2: Capture UI

### Почему блокер

`shell/capture.rs` и `logic/region_editor.rs` вызывают backend-функции capture/render/save
в UI-процессе. Backend protocol готов, но overlay не использует.

### Задачи

#### C1. Blocking capture client
Файл: `frontend/src/logic/capture_client.rs`.
- отдельное блокирующее IPC-соединение к daemon;
- `capture_monitor(index) -> CaptureSourceWire` (или window);
- `commit(ScreenshotCommitWire)`.
- Не использовать `mediaa_backend::capture`.

#### C2. Overlay на source-файл
Файлы:
- `shell/capture.rs`
- `logic/region_editor.rs`
- `ui/region_editor.rs`

- прочитать `.rgba` по path + header;
- загрузить текстуру;
- selection/annotations — native coordinates;
- финал: компоновать `mediaa_render::annotate` в native resolution;
- записать RGBA во временный файл по backend-issued пути/id;
- отправить `screenshot_commit`;
- результат/ошибку показать.

#### C3. Fullscreen/window без overlay
- backend capture + commit напрямую, UI только результат.
- Region — source file + overlay.

Acceptance:
- UI не вызывает `mediaa_backend::capture::*`;
- нет `Frame` из backend API в UI;
- PNG/WebP;
- clipboard backend.

## 6. Удаление зависимостей frontend

`crates/frontend/Cargo.toml`:

Убрать normal:
- `mediaa-backend`
- `mediaa-storage`
- `mediaa-agent`
- `mediaa-engine`
- `mediaa-queue`
- `global-hotkey` (если парсинг хоткеев уйдёт в backend)

Dev:
- `mediaa-storage` — только если тесты реально требуют; лучше убрать.

Проверка:

```powershell
cargo tree -p mediaa-frontend -e normal
cargo tree -p mediaa-backend -e normal
```

Arch:
- `crates/arch/src/lib.rs`: frontend strict `normal: ["core","sync","protocol"]` (плюс media, если
  read-only playback остаётся; зафиксировать).
- app strict `["frontend","backend"]`.
- убрать все temporary комментарии/allow.

## 7. Legacy cleanup

Удалить/переместить:
- `frontend/src/logic/conversion` test-only job/probe — оставить только UI view/protocol или перенести тесты;
- `frontend/src/logic/recording/controller.rs`, `host.rs` test-only — удалить, тесты на protocol;
- `frontend/src/adapters/tray_types` — если tray полностью backend, типы брать из protocol;
- `frontend/src/logic/notices` re-export — оставить только нужное;
- `crates/app` — не возвращать UI/adapters/logic;
- старые `specs/backend-split.md`/`backend-split-plan.md` — пометить superseded, оставить final-split;
- `thumb_worker.rs` — уже удалён, не возвращать;
- `cache_maintenance` — уже удалён;
- `app/.cache`, старые snapshots — не коммитить.

Backend:
- не должен зависеть от frontend/egui;
- tray/hotkeys/settings остаются backend;
- i18n-заглушки для headless — допустимы, но не тянуть frontend;
- `mediaa_backend::daemon` — единственный backend runtime.

## 8. Тесты

Обязательно:
- `cargo test -p mediaa`
- `cargo test -p mediaa-frontend --lib`
- `cargo test -p mediaa-backend`
- `cargo test -p mediaa-protocol`
- `cargo test -p mediaa-arch`
- `cargo clippy -p mediaa -p mediaa-frontend -p mediaa-backend --lib -- -D warnings`

Интеграция:
- два процесса: `--backend` + `--ui`;
- reconnect UI;
- preview request → event → texture;
- recording start/stop;
- conversion start/progress/result;
- agent send → snapshot/live/done;
- capture region → source file → commit → clipboard;
- закрытие UI не убивает backend;
- launcher повторно открывает UI.

Snapshots:
- `crates/frontend/tests/snapshots` — путь сохранить;
- flaky locale-тесты — использовать `test_support::english_locale`.

## 9. Замеры и документация

Обновить:
- `specs/tasks/backend-split-measurements.md` (release, idle UI/backend, запись, previews, conversion);
- `specs/progress.md`;
- `specs/tasks/final-split.md`;
- `specs/architecture.md`, если структура изменилась.

Текущий ориентир (release):
- backend ≈ 15.7 MB;
- UI ≈ 503 MB (egui/wgpu);
- лимитов памяти нет.

## 10. Definition of Done

- `cargo tree -p mediaa-frontend` без backend/storage/agent/engine/queue;
- arch strict;
- UI не импортирует `AgentServices`, `Scheduler`, `Store`, `Streams`, `Engine`;
- UI не вызывает capture/render/save backend-функции напрямую;
- app только launcher/роли;
- backend отдельный процесс;
- все тесты/clippy;
- release smoke двух процессов;
- documentation/measurements обновлены;
- нет legacy/test-only production кода.

## 11. Порядок работ

1. A1 queue snapshot.
2. A2 чаты/каталоги.
3. A3 live timeline.
4. A4 удалить AgentView/AgentServices.
5. C1 capture client.
6. C2 overlay commit.
7. C3 fullscreen/window.
8. Удалить зависимости frontend + arch.
9. Legacy cleanup.
10. Тесты/замеры/доки.

## 12. Ловушки

- Не возвращать `AgentHost`/handoff в UI.
- Не передавать пиксели по IPC: только source/result id/path и метаданные.
- Не использовать egui framebuffer для финального скриншота — только native `mediaa_render`.
- `ImageFormat` только Png/Webp.
- `MAX_FRAME_BYTES` — причина не гонять кадр по JSON.
- Rust orphan rule: UI-методы на внешних типах — только local traits.
- В app `src/icon.rs` нужен `build.rs`; не удалять.
- DB reset допустим; миграций нет.
- Не коммитить без разрешения.
