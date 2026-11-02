# ST-1: App lifecycle — окно, трей, выход

## Текущий срез

Сделано:

- `mediaa-protocol::state::tray` — reducer, 14 тестов;
- backend `Daemon` использует `TrayMachine`, поля `show_retry_at`,
  `show_ack_pending`, `spawned_at` удалены;
- frontend `WindowLifecycle` — `Visible/Hidden/Quitting`, 10 тестов;
- удалены `show_on_start`, `quit_requested`, `idle_since`;
- `presence.hidden` удалён, hidden живёт в `WindowLifecycle`;
- ранний `TRAY_SHOW_ACK` из `App::new`, чтобы UI не перезапускался по
  таймауту;
- `TRAY_HIDE_ACK` обновляет состояние окна;
- немедленный `Quit` из трея при простое.

Итог:

- backend smoke после frontend-рефакторинга выполнен;
- полный suite не является частью этой задачи.

## Симптом

Первый клик «Открыть» поднимает UI, который сам завершается; второй клик
поднимает рабочий. «Выход» иногда требует двух кликов. Логи:

```text
22:12:16.373 tray menu event: show
22:12:17.254 tray command: ShowWindow, ui=false
22:12:17.273 UI starting
22:12:19.062 ui exiting: close_to_tray=true daemon control=true
22:12:22.705 tray menu event: show
22:12:23.401 UI starting
```

Первый UI не «не открылся» — он поднялся и завершился. Второй клик не
исправляет первый, а создаёт новый процесс.

## Текущее состояние

### Backend `Daemon`

- `ui: Option<Connection>`
- `spawned_at: Option<Instant>`
- `show_retry_at: Option<Instant>`
- `show_ack_pending: bool`
- `pending_events: Vec<(String, Vec<u8>)>`
- `recording_quit_when_idle: bool`

### Backend `Tray`

- `applied: Option<(TrayState, bool, String)>`
- `TrayState` — это состояние записи, не lifecycle окна.

### Frontend `App`

- `presence.hidden`
- `startup: StartupAction`
- `show_on_start: bool`
- `quit_when_idle: bool`
- `quit_requested: bool`
- `idle_since: Option<Instant>`
- `tray_applied: Option<(TrayState, bool)>`

### Уже подготовлено

`crates/backend/src/daemon/tray.rs` — черновик `TrayMachine`, ещё не
подключён. Его нужно либо довести, либо удалить; он не должен существовать
как мёртвый код.

## Целевая модель

### Backend `TrayMachine`

Состояния:

```rust
enum TrayPhase {
    Absent,
    Starting,
    Opening,
    Open,
    Hidden,
    Quitting,
}
```

События:

```rust
enum TrayEvent {
    ClickShow,
    ClickHide,
    ClickQuit,
    UiConnected,
    UiDisconnected,
    WindowShown,
    WindowHidden,
    Tick(Instant),
    QuitReady,
}
```

Эффекты:

```rust
enum TrayEffect {
    SpawnVisible,
    SpawnHidden,
    SendShow,
    SendHide,
    SendQuit,
    RestartVisible,
    FinishQuit,
}
```

### Frontend `WindowLifecycle`

```rust
enum WindowLifecycle {
    Visible,
    Hidden { since: Instant },
    ClosingToTray,
    Quitting,
}
```

Переходы:

- `show_window`: `Hidden/ClosingToTray -> Visible`, отправить `WindowShown`.
- `hide_window`: `Visible -> Hidden { since }`, отправить `WindowHidden`.
- `on_close`: `Visible -> ClosingToTray`, скрыть, отправить `WindowHidden`.
- `quit`: `Any -> Quitting`, отправить `QuitReady`.
- `tick(now)`: `Hidden { since }` + 3 s -> `Closing`.
- `show_window` сбрасывает `since`, поэтому старый таймер не закрывает
  только что открытое окно.

## Таблица переходов backend

| Состояние | Событие | Новое | Эффект |
|---|---|---|---|
| Absent | ClickShow | Starting | SpawnVisible |
| Starting | UiConnected | Opening | — |
| Opening | WindowShown | Open | — |
| Opening | UiDisconnected | Starting | RestartVisible, один раз |
| Opening | Tick(deadline) | Starting | RestartVisible, один раз |
| Open | ClickHide | Hidden | SendHide |
| Hidden | ClickShow | Opening | SendShow |
| Hidden | UiDisconnected | Absent | — |
| Open/Hidden | ClickQuit (idle) | Quitting | FinishQuit |
| Open/Hidden | ClickQuit (busy) | Quitting | SendQuit |
| Quitting | QuitReady | — | FinishQuit |

Повторный `ClickShow` в `Starting/Opening` не создаёт второй процесс.
Повторный `ClickQuit` в `Quitting` ничего не делает.

## Что удалить

Backend:

- `show_retry_at`, `show_ack_pending`;
- `spawned_at` (заменить состоянием `Starting`);
- `pending_events` (машина хранит очередь эффектов);
- `recording_quit_when_idle` (вынести в `QuitPolicy`).

Frontend:

- `show_on_start`;
- `quit_requested`;
- `idle_since` (перенос в `Hidden { since }`);
- `startup` как runtime-флаг (оставить только аргумент запуска).

## Protocol

Добавить явные события:

- `TRAY_SHOW_ACK` — UI показал окно;
- `TRAY_HIDE_ACK` — UI скрыл окно;
- `TRAY_READY` — UI готов;
- `TRAY_QUIT_READY` — UI завершил запись и готов выйти.

## Тесты

### Backend `TrayMachine` (без Windows)

1. `show_from_absent_spawns_one_visible_ui`
2. `second_show_does_not_spawn_second_ui`
3. `show_from_hidden_sends_show`
4. `ui_disconnect_while_starting_restarts_once`
5. `lost_ack_restarts_once`
6. `restart_does_not_loop`
7. `quit_when_idle_finishes_immediately`
8. `quit_while_busy_waits_for_ready`
9. `quit_is_terminal`
10. `hide_then_show_cancels_the_idle_timer`

### Frontend `WindowLifecycle`

1. `hidden_becomes_visible_and_emits_shown`
2. `visible_becomes_hidden_and_emits_hidden`
3. `hidden_after_three_seconds_closes`
4. `show_before_timeout_cancels_close`
5. `quit_from_any_state_emits_quit_ready`
6. `close_while_quitting_is_ignored`

### Integration

Фейковый runtime без ОС:

```rust
runtime.click(Show);
assert_eq!(runtime.effects(), [SpawnVisible]);
runtime.ui_connected();
runtime.window_shown();
assert_eq!(runtime.phase(), Open);
```

## Критерии готовности

- Один клик «Открыть» → `Open`, ровно один UI.
- UI умер при открытии → автоматический restart, не второй клик.
- Один клик «Выход» при простое → процесс завершён.
- В backend нет `show_retry_at`, `show_ack_pending`, `spawned_at`.
- Во frontend нет `show_on_start`, `quit_requested`, `idle_since`.
- Все переходы таблицы покрыты тестами.
