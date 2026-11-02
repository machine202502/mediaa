# FNL: финальная чистка разделения

Статус: 03.11.26. После переноса UI в `mediaa-frontend` нужно убрать старые файлы
и вынести из крейтов то, что там не должно жить.

Правило работы: крупный шаг → сборка/тесты → следующий шаг. Не дробить на микропатчи.

## Целевое состояние

```text
crates/app       только main + launcher-роли
crates/backend   headless: daemon, БД, запись, capture, previews, conversion jobs,
                 settings/tray/hotkeys, agent host/stream
crates/frontend  весь UI, view-модели, transport-клиент, чтение файлов
```

`frontend` не зависит от `backend`, `storage`, `agent`, `engine`, `queue`.
Общение — только `protocol`/`sync`.

## Задачи

### FNL-1. Удалить старый app
- удалить `crates/app/src/{adapters,logic,shell,ui,daemon.rs,wire.rs,...}`;
- оставить только `main.rs` и `build.rs`;
- удалить старые `crates/app/tests`, `locales`, `.cache`, если не используются;
- `crates/app` собирается как один тонкий bin.
**Готово:** в app нет UI/backend-кода, `cargo build -p mediaa` зелёный.

### FNL-2. Frontend runtime без backend-daemon
- заменить `frontend/src/daemon.rs` на `ui_session.rs` только с `run_ui`;
- удалить из frontend backend-части: `run`, `launch`, acceptor, settings/tray/hotkeys;
- `entry.rs` использует `backend::daemon::launch/run` и `ui_session::run_ui`;
- удалить frontend `adapters/settings_file.rs`, `tray.rs`, `hotkeys.rs`, если UI их не использует.
**Готово:** frontend не содержит backend-daemon логики; launcher/backend — в app/backend.

### FNL-3. Frontend: убрать backend-сервисы
- удалить `logic/cache_maintenance.rs` и фоновый cache-sweep из UI — это backend;
- удалить production `thumb_worker` (тестовый заменить моком);
- preview/cleanup — только backend;
- clipboard: UI получает сервис через protocol/или backend-owned, не создаёт.
**Готово:** frontend не чистит кэш, не генерирует превью, не владеет clipboard-сервисом.

### FNL-4. Frontend: library view
- `LibraryController` — только `LibraryView` + команды;
- query/filter/sort — `library_query` или чистый snapshot;
- убрать `LibraryIndex`/`mediaa-storage` из frontend полностью.
**Готово:** `cargo tree -p mediaa-frontend` без storage.

### FNL-5. Frontend: conversion view
- `ConversionController` — форма + `ConversionStateWire`;
- старт/прогресс/ошибки — backend;
- удалить локальные conversion jobs/probe из frontend.
**Готово:** conversion не запускает media jobs во frontend.

### FNL-6. Frontend: agent view
- `AgentHost`/`AgentServices`/Scheduler/Store — удалить из frontend;
- чаты/каталоги читать с диска;
- send/stop/resume — protocol;
- live timeline — `agent` события с `task`.
**Готово:** frontend не зависит от agent/engine/queue.

### FNL-7. Frontend dependencies
- `cargo tree -p mediaa-frontend` без backend/storage/agent/engine/queue/media-энкодеров;
- arch-правило frontend снова строгое;
- удалить временные allow.
**Готово:** arch test зелёный без временных правил.

### FNL-8. Backend cleanup
- удалить из backend UI-типы, i18n-сообщения, icon, tray-иконки, если не нужны headless;
- tray/hotkeys/settings оставить backend;
- conversion/recording/capture/preview — backend;
- backend не зависит от frontend.
**Готово:** `cargo tree -p mediaa-backend` без frontend/egui.

### FNL-9. Удаление legacy
- старые app-файлы удалены;
- дубли recording/conversion/agent удалены;
- тестовые worker’ы заменены;
- build-артефакты и старые snapshots не мешают.
**Готово:** нет мёртвых модулей/файлов, clippy чистый.

### FNL-10. Финальные тесты/замеры/доки
- unit + integration;
- два процесса, reconnect, preview queue, agent stream;
- release RSS/CPU;
- обновить progress/architecture.
**Готово:** тестовая матрица и отчёт.

## Очередь

1. FNL-1 — app cleanup.
2. FNL-2 — frontend runtime cleanup.
3. FNL-3 — backend services removed from frontend.
4. FNL-4 — library view.
5. FNL-5 — conversion view.
6. FNL-6 — agent view.
7. FNL-7 — dependencies/arch.
8. FNL-8 — backend cleanup.
9. FNL-9 — legacy removal.
10. FNL-10 — tests/docs/measurements.
