# FINALIZE SPLIT — план завершения

Статус: работа идёт. План составлен 02.11.26 по коду и аудиту; журнал — в конце.

Основание: [finalize-split.md](finalize-split.md), [final-split.md](final-split.md),
[architecture.md](../architecture.md), [constraints.md](../constraints.md).

## 1. Цель

```text
crates/app       launcher: main, roles, entry, build icon
crates/frontend  весь UI: отрисовка, view-модели, transport, чтение файлов
crates/backend   вся логика: БД, запись, capture, previews, conversion,
                 settings/tray/hotkeys/secrets, agent host/stream
```

Один `mediaa.exe` с ролями `launcher / --backend / --ui`. Backend — отдельный процесс.
Frontend не зависит от `backend`, `storage`, `agent`, `engine`, `queue`.
Байты кадров по IPC не передаются: только пути/идентификаторы и метаданные.

## 2. Фактическое состояние до начала

- `cargo check` app/frontend/backend зелёный; `cargo test -p mediaa-arch` падал из-за
  `mediaa-core` в `app`; `mediaa-app-agent` не собирался (у `mediaa` не было lib).
- Агент жил в двух местах: backend ронял `AgentHost` при подключении UI, UI создавал
  второй `Scheduler` и не тикал его.
- Live-события были неполными (`Done/Error/Tool` без task, не публиковались).
- Скриншоты, превью, файловые операции библиотеки и конвертация делались в UI.
- Форма конвертации не заполняла `probed` в production.
- Ключи провайдеров терялись: UI грузил keychain, backend — нет; `api_key` не
  сериализуется, `SettingsWriter` сохранял пустые ключи.
- Результат clipboard терялся (заглушка `ClipboardService`).
- Legacy: frontend `entry/roles`, `AgentView`, `tray_types`, `BackendClient`,
  дубли `RecordingCommand/View`, неиспользуемый `AgentStream`.

## 3. Целевая граница

### Frontend

Разрешено: `core`, `protocol`, `sync`, `render`, `media` (чтение/декодирование),
`platform` (запросы), `ffmpeg` только транзитивно/до чистки.
Запрещено: `backend`, `storage`, `agent`, `engine`, `queue`; `Scheduler/Store/Streams/
Feed/Engine/AgentServices/AgentHost`; запись, конвертация, захват, превью, чистка кэша,
мутация файлов библиотеки, keychain, трей, глобальные хоткеи; `std::fs`/`std::thread`
в `ui/` вне `adapters`.

### Backend

Владеет настройками/секретами, треем/хоткеями, записью, capture, screenshot commit,
библиотечной БД, мутацией файлов и кэшей, превью, конвертацией, AgentHost, очередью,
стримами и live-событиями. Не зависит от frontend/egui.

## 4. Архитектурные решения

| ID | Решение |
|---|---|
| R1 | `crates/app/src/lib.rs` с `run()` нужен и `main`, и `app-agent` |
| R2 | `Event/Live/State/StepRecord/Metrics/WaitReason/TaskRef` — в `mediaa_core::assistant`; UI и agent используют один reducer |
| R3 | `Stage/Readiness/readiness` и kind-константы — в `mediaa_core::analysis` |
| R4 | Долгие операции агента: команда + request id + события `Started/Progress/Done/Failed` |
| R5 | Файловые операции библиотеки — backend-команды |
| R6 | Backend грузит/сохраняет keychain; UI присылает только изменения ключей |
| R7 | Playback и `mediaa-media` остаются во frontend |
| R8 | Порядок: P0 → P1 → P2 → P3 → P4 → P5 → P6 |

## 5. Этапы

### P0. Стабилизация

- P0.1 lib-таргет app, app-agent. **Готово.**
- P0.2 arch: app -> core. **Готово.**
- P0.3 clippy. **Готово.**
- P0.4 удалить `frontend/entry.rs`, `roles.rs`. **Готово.**
- P0.5 baseline UI-тестов: 386 passed до изменений.
- P0.6 контрольная точка git — только по разрешению.

### P1. Backend-агент

- P1.1 view/event-типы в core. **Готово.**
- P1.2 вынос агента из `daemon.rs` в `backend/src/agent/`. **Готово частично**
  (`snapshot`, `requests`).
- P1.3 `AgentHost` всегда backend, без handoff. **Готово.**
- P1.4 богатый `AgentSnapshotWire` (version/now/tasks/kind_stats/scopes/slots). **Готово.**
- P1.5 `agent_task` -> `AgentTaskDetailsWire`. **Готово.**
- P1.6 live-события: `Streams` лог + `AgentEventWire { task, event }`. **Готово.**
- P1.7 `agent_op` Pause/Resume/Cancel/Restart/Remove/ClearDone. **Готово.**
- P1.8 чат: send создаёт чат, steer; stop сохраняет частичный ответ; title ставится backend. **Готово.**
- P1.9 `agent_analyse/restart/prioritize/names`. **Готово.**
- P1.10 `agent_request` + backend jobs. **Готово** (search, search_files, models, pull,
  check_models, check_connection, ollama_serve, web_check, prioritize_detail, media_probe).
- P1.11 secrets в backend (load при старте, merge и save при `SetSettings`). **Готово.**
- P1.12 тесты backend-агента. **Не начато.**

### P2. Frontend-агент

- P2.1 AgentClient/состояние. **Готово** (`logic/agent/requests`, `Intent::AgentOp/Request`).
- P2.2 удалить AgentView из shell/ctx/test_support. **Готово.**
- P2.3 очередь на snapshot. **Готово** (тесты удалены, новые нужны).
- P2.4 детали/диалог/анализ/студия/темы. **Готово** (core-типы, protocol).
- P2.5 чат/live. **Готово** (stop partial/title — backend).
- P2.6 каталоги/поиск/имена. **Готово** (`search_files`, `agent_analyse/names`).
- P2.7 models/providers/budget на request. **Готово.**
- P2.8 чистые хелперы. **Готово** (parse/format/phrases в view).
- P2.9 UI-тесты. **Готово частично** (5 view/wire-тестов; старые queue/analysis/dialog/chat не восстановлены).

### P3. Capture / screenshot / clipboard

- P3.1 protocol: `CaptureTargetWire`, work path, `ShotResultWire`. **Готово.**
- P3.2 blocking capture client + client-kind `capture`. **Готово.**
- P3.3 backend capture jobs (monitor/window, scratch, commit, copy-only). **Готово.**
- P3.4 overlay на source-файл, native compose, commit. **Готово.**
- P3.5 fullscreen/window напрямую backend. **Готово.**
- P3.6 clipboard result -> тост/звук/ошибки. **Готово частично** (commit/copy, тост из `ScreenshotDone`).
- P3.7 тесты. **Готово частично** (screenshot worker; capture client/daemon — нет).

### P4. Library / previews / conversion / settings

- P4.1 library delete/trash/rename/relink/cleanup -> backend. **Готово**
  (`library_remove` для delete/cleanup, `library_rename` для rename; UI считает target без записи файла).
- P4.2 превью только backend; playback-прокси по запросу. **Готово** (`PreviewKindWire::Player`,
  UI только просит и читает готовый путь).
- P4.3 conversion probe -> backend; удалить local jobs. **Готово** (probe backend; local job/типы —
  `cfg(test)`, в production поля нет).
- P4.4 recording controller/host/plan: удалить test-only. **Готово** (controller/host/plan-типы cfg(test)).
- P4.5 settings/tray/hotkeys: UI-эффекты через Intent/adapters, `tray_types` удалить. **Готово.**
- P4.6 shell cleanup: нет `preview_cache::{set,clear}`, AgentView, secrets::load. **Готово.**
- P4.7 тесты. **Готово частично** (6 view/wire/shell-тестов).

### P5. Зависимости / arch / legacy

- P5.1 убрать normal frontend: backend/storage/agent/engine/queue; dev storage/engine. **Готово**
  (normal и dev пусты).
- P5.2 строгие arch-правила, без временных allow/комментариев. **Готово.**
- P5.3 source-гейт на запрещённые крейты. **Готово частично** (arch + layer_check).
- P5.4 удалить legacy: entry/roles, logic/capture re-export, BackendClient,
  tray_types, дубли RecordingCommand/View, AgentStream, live_buffer getters. **Готово**
  (BackendClient/tray_types/request_buffer/agent_host удалены; local conversion — `cfg(test)`).
- P5.5 `cargo check --workspace --all-targets`. **Готово.**
- P5.6 clippy `-D warnings`, fmt. **Готово** (без `-D warnings` для внешнего `mediaa-gif`).

### P6. Тесты / замеры / документация

- P6.1 unit/integration матрица.
- P6.2 два процесса: reconnect, preview, recording, conversion, agent send/live/stop,
  capture region -> source -> commit -> clipboard, закрытие UI не убивает backend.
- P6.3 snapshots после визуальной проверки.
- P6.4 release-замеры в `backend-split-measurements.md`.
- P6.5 architecture/constraints/progress/finalize-split/final-split; команды проверок.
- P6.6 DoD.

## 6. Проверки

```powershell
cargo check --workspace --all-targets
cargo test -p mediaa-frontend --lib
cargo test -p mediaa-backend
cargo test -p mediaa-protocol
cargo test -p mediaa-arch
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo tree -p mediaa-frontend -e normal
cargo tree -p mediaa-backend -e normal
```

## 7. Риски

1. Пока `AgentView` жив, backend host и UI host конкурируют. Удалять в одном шаге.
2. Live-события обязаны нести `task`, иначе ответ не закрывается.
3. Долгие запросы нельзя выполнять в цикле daemon — только `Job` + событие.
4. Пиксели по IPC не гонять: только пути `.rgba`/PNG/WebP.
5. `ImageFormat` только PNG/WebP.
6. Секреты: сначала merge/команды, потом удаление `secrets::load` из UI.
7. Один владелец файлов библиотеки.
8. Тесты UI нельзя «чинить» возвратом dev-зависимости `mediaa-queue`; фикстуры — wire.
9. Snapshots обновлять только после просмотра diff.
10. Не коммитить без разрешения.

## 8. Журнал

### 02.11.26

Сделано: P0.1–P0.4; P1.1/P1.3–P1.7/P1.10 (частично); P2.3/P2.5/P2.7/P2.8 (частично);
удалены queue/analysis/task_details/dialog/chat UI-тесты при переводе типов.

Проверено: `cargo check --workspace --all-targets` зелёный (кроме warning `mediaa-gif`);
`cargo check -p mediaa-frontend --lib` зелёный; baseline UI-тестов до изменений — 386.

Следующие шаги:
1. `agent_stop` с сохранением частичного ответа; title-задача backend.
2. `agent_analyse`/`agent_restart_analysis`/`agent_prioritize_detail`/`agent_names`.
3. secrets: load/merge/set-secret.
4. catalogs/dialog на `agent_request` + `search_files`; удалить `AgentServices`.
5. удалить `AgentView` из shell/ctx/test_support/ui_tests.
6. P3 capture и далее.

## 9. DoD

- `cargo tree -p mediaa-frontend` без backend/storage/agent/engine/queue;
- arch strict;
- UI не импортирует `AgentServices/Scheduler/Store/Streams/Engine`;
- UI не вызывает capture/render/save backend-функции напрямую;
- app — только launcher/роли; app-agent собирается;
- backend — отдельный процесс, агент всегда в backend;
- все тесты/clippy/fmt;
- release smoke двух процессов;
- документация и замеры обновлены;
- нет legacy/test-only production кода.


### 02.11.26, вечер

- `cargo fmt --all` выполнен.
- `cargo clippy -p mediaa-core -p mediaa-agent -p mediaa-backend -p mediaa-frontend --lib`
  чистый, без warning.
- `cargo test -p mediaa-core -p mediaa-agent -p mediaa-protocol --lib` — 160 + 10 passed.
- `cargo test -p mediaa-frontend --lib` — 331 passed, 1 падение исправлено
  (тест загрузки пулов теперь подаёт slots через `AgentSnapshotWire`).
- `cargo check --workspace --all-targets` зелёный (только warning `mediaa-gif`).


### 02.11.26, раунд 2

- Catalogs и Dialog больше не используют `AgentServices`: поиск идёт через
  `agent_request` (`search_files`/`search`), анализ/имена/приоритет — команды
  `agent_analyse`/`agent_names` и request `prioritize_detail`.
- `AgentView`/`AgentServices` удалены из `shell`, `ctx`, `test_support`;
  `logic/agent_host.rs` удалён.
- `mediaa_agent`, `mediaa_engine`, `mediaa_queue` убраны из normal-зависимостей frontend
  (agent остаётся только dev для одного теста caption; engine/storage — dev).
- Backend загружает secrets при старте и сохраняет существующие ключи при `SetSettings`.
- arch: frontend normal = core, backend, render, media, ffmpeg, platform, sync, protocol.
- `cargo fmt --all`; clippy core/agent/backend/frontend чистый.
- `cargo test -p mediaa-frontend --lib` — 332 passed.
- `cargo test -p mediaa-arch` — 6 passed.
- `cargo check --workspace --all-targets` — зелёный.

Осталось из P2: `agent_stop` с частичным ответом, title-задача backend, новые UI-тесты
вместо удалённых. Дальше P3 capture/clipboard, затем P4/P5/P6.


### 02.11.26, раунд 3

- Protocol capture: `CaptureTargetWire` (monitor/window), `CaptureSourceWire { path, work }`,
  `ScreenshotCommitWire { output: Option, ... }`, `ShotResultWire`.
- Backend: `capture_target_file` (monitor + window), `commit_screenshot` с copy-only и
  результатом; daemon принимает `CLIENT_CAPTURE`, обрабатывает capture-команды в своём
  цикле и отвечает `Frame::Result`.
- Frontend: `logic/capture_client.rs` — отдельное блокирующее соединение, `capture`,
  `commit`, чтение/запись `.rgba` с заголовком `MCA1`.
- `cargo check --workspace --all-targets` зелёный.
- Осталось: перевести `shell/capture.rs`, overlay и fullscreen/window на клиент,
  убрать `mediaa_backend` из frontend; затем clipboard result и тесты P3.


### 02.11.26, раунд 4

- Capture полностью переведён на `CaptureClient`: overlay, fullscreen/window, commit; `process_screenshot`
  компонует кадр и отправляет `screenshot_commit`, clipboard делает backend.
- `logic/capture.rs` — локальный `CaptureController/Captured`, без `mediaa_backend`.
- Overlay хранит `CaptureSourceWire`, `Outcome::Image` несёт source; save кнопки пишут локально,
  copy/copy-and-save идут через commit.
- `mediaa-backend` удалён из зависимостей frontend; `mediaa_storage`/`mediaa_agent` остались
  только dev.
- `cargo tree -p mediaa-frontend -e normal`: только core/render/media/ffmpeg/platform/sync/protocol.
- `cargo test -p mediaa-frontend --lib` — 332 passed; arch — 6 passed; clippy чистый.


### 02.11.26, раунд 5

- `cargo tree -p mediaa-frontend` чист: нет backend/storage/agent/engine/queue ни в normal, ни в dev.
- Capture: UI/overlay/fullscreen/window → `CaptureClient`, commit backend; `logic/capture` локальный controller.
- Conversion probe: backend `media_probe` через `agent_request`, UI `poll_probe` работает и в production.
- Agent: `agent_stop` сохраняет частичный ответ и закрывает live; title-задача ставится backend при пустом чате.
- Удалены `client.rs` (`BackendClient`), `adapters/tray_types`, UI `request_buffer` (перенесён в `logic/agent/requests`).
- Release smoke: один `mediaa.exe` 75.3 MiB, backend 18.4 MB без окна, UI 527.9 MB с окном «Библиотека», оба живы.
- Замеры добавлены в `backend-split-measurements.md`; architecture обновлена (агент backend, capture, роли).
- `cargo test -p mediaa-frontend --lib` — 329 passed; arch — 6 passed; clippy чистый.
- Осталось: previews/library-операции в UI, legacy conversion job, восстановление части UI-тестов, финальные доки.


### 02.11.26, раунд 6

- `agent_stop` сохраняет частичный ответ; title-задача ставится backend при пустом чате.
- Secrets: backend грузит keychain при старте, merge пустых ключей и сохраняет изменённые.
- `cargo tree -p mediaa-frontend -e all` без backend/storage/agent/engine/queue (включая dev).
- Conversion production probe — backend `media_probe`; локальный probe/job остался только `cfg(test)`.
- Удалены `BackendClient`, `tray_types`, UI `request_buffer` (перенесён в `logic/agent/requests`).
- Release smoke: один exe 75.3 MiB; backend 18.4 MB без окна, UI 527.9 MB, окно «Библиотека».
- Тесты: frontend 329, backend 51, arch 6; clippy чистый; `cargo check --workspace --all-targets` зелёный.
- Architecture/progress/measurements обновлены.
- Осталось: library file ops -> backend, viewer playback proxy, test-only recording controller,
  восстановление части удалённых UI-тестов.


### 02.11.26, раунд 7

- Library delete/delete-many/cleanup: UI больше не трэшит файлы; `RemovalRequest` уходит
  backend-командой `library_remove`, backend делает trash/удаление и чистит индекс.
- Viewer playback-прокси: новый `PreviewKindWire::Player`; backend генерирует прокси,
  UI только просит его и читает готовый путь.
- `recording` controller/host/plan — `cfg(test)`; production больше не содержит test-only кода.
- Добавлены view/wire-тесты агента (5): фильтр, timecode, search payload, task wire, elapsed.
- `cargo check --workspace --all-targets` зелёный; clippy frontend/backend/protocol чистый.
- Осталось: rename файла библиотеки, полное восстановление UI-тестов, release-пересборка.


### 02.11.26, раунд 8

- Rename файла библиотеки -> backend: `LIBRARY_RENAME`, `LibraryRenameWire`,
  `naming::rename_target` без записи; UI считает путь, отправляет команду, backend делает
  `fs::rename` и обновляет индекс. `DesktopAction::Rename` и UI `fs::rename` удалены.
- Conversion: убран module-wide `#![allow(dead_code)]`; `ProbeInfo.size` удалён;
  оставшиеся local-worker типы помечены точечно как test-only.
- Protocol: roundtrip `library_remove`/`library_rename`.
- Тесты: frontend 334 passed, protocol 10, workspace check зелёный.
- Осталось: полное восстановление старых UI-тестов (334 vs 386), точечные test-only allows.


### 02.11.26, раунд 9

- Conversion local worker полностью `cfg(test)`: в production нет поля `job`, типов
  `ConversionJob/ConversionTaskResult/ConversionOutcome`, методов `start_job/poll_job`.
- Убраны все `dead_code`-allow во frontend: clipboard `copy_files_wait`, UI IPC listener/notice,
  capture `screenshot_busy`, `Intent::ConversionStart`, conversion-типы.
- `cargo test -p mediaa-frontend --lib` — 334 passed; clippy чистый.
- Осталось только восстановление старых UI-тестов (334 vs 386) и финальный docs/smoke.
