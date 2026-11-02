# ST-8: App как composition root, а не мешок полей

## Ответ на вопрос

Нет, это не адекватно.

`App` сейчас:

- 45 полей;
- 858 строк в `app.rs`;
- `app/actions.rs` — 38 KB;
- `app/ui_tests.rs` — 123 KB;
- одновременно владеет окном, треем, daemon IPC, каталогами, записью,
  захватом, библиотекой, превью, агентом, конвертацией, настройками,
  восстановлением, jobs, dev-input и тестовыми подменами.

Это не «composition root», а god object. Поля не сгруппированы по
ответственности, поэтому любой баг требует знать весь `App`.

## Целевая структура

`App` оставляет только:

```rust
pub struct App {
    shell: AppShell,
    window: WindowController,
    daemon: DaemonBridge,
    recording: RecordingUi,
    capture: CaptureUi,
    library: LibraryUi,
    agent: AgentUi,
    conversion: ConversionUi,
    notices: Notices,
    settings: SettingsUi,
    catalogs: Catalogs,
    input: InputState,
}
```

### `WindowController`

Владеет:

- `WindowLifecycle { Visible, Hidden { since }, ClosingToTray, Quitting }`;
- `TrayPhase`;
- native window;
- tray ack/effects.

Не знает про recording/library/agent.

### `DaemonBridge`

Владеет:

- `Connection`;
- `Client`;
- `next_command`;
- pending request map;
- `daemon_lost`;
- входящие receivers.

API: `send(command)`, `poll() -> Vec<DaemonEvent>`.
Не знает про UI-страницы.

### `RecordingUi`

Владеет:

- `RecordingController`;
- `RecordingViewState`;
- `pending` recording action;
- recording-specific effects.

### `CaptureUi`

Владеет:

- `CaptureState`;
- editor/overlay;
- capture jobs.

### `LibraryUi`

Владеет:

- `LibraryController`;
- `Thumbnails`;
- previews/player/viewer;
- library cache.

### `AgentUi`

Владеет:

- `AgentUiState` (snapshot/live/requests);
- tabs/queue;
- agent channels.

### `ConversionUi`

Владеет:

- `ConversionState`;
- batch;
- probe.

### `SettingsUi`

Владеет:

- settings;
- dirty;
- hotkey problems;
- settings write.

### `Catalogs`

Владеет:

- monitors/windows/mics;
- catalog job;
- desktop events.

### `InputState`

Владеет:

- dev input;
- raw input dispatch.

## Правила

- Контроллер не знает про `egui::Context`, кроме метода `ui(ctx)`.
- Контроллер не читает `App`.
- Переходы контроллера тестируются без `Harness`.
- `App::frame_ui` только вызывает `controller.ui(...)`.
- `App::logic` только опрашивает контроллеры и применяет эффекты.

## Порядок

1. Вынести `WindowController` и `DaemonBridge` (самые связанные с треем).
2. Вынести `RecordingUi`/`CaptureUi`.
3. Вынести `LibraryUi`/`AgentUi`/`ConversionUi`.
4. Вынести `Notices`/`SettingsUi`/`Catalogs`.
5. Удалить поля-дубли из `App`.
6. `app.rs` должен остаться < 250 строк.

## Критерии готовности

- В `App` не больше 12 полей-контроллеров.
- `app.rs` < 250 строк.
- Каждый контроллер имеет свои unit-тесты без egui.
- Нет поля, которое одновременно относится к двум доменам.
- `show_on_start`, `quit_requested`, `idle_since` не существуют.
