# ST-2: Recording — одно состояние вместо RecState/ViewState/phase

## Где

- `crates/backend/src/features/recording/controller.rs`
- `crates/backend/src/features/recording/host.rs`
- `crates/backend/src/features/recording/wire.rs`
- `crates/frontend/src/features/recording/controller.rs`
- `crates/frontend/src/features/recording/types.rs`
- `crates/frontend/src/features/recording/host.rs`

## Текущее состояние

Сейчас одновременно существуют:

- `RecState` (backend и frontend) — `Idle/Starting/Recording/Saving`;
- `RecordingView` — те же фазы, но с данными для UI;
- `RecordingPhase` — выводится из `RecordingView`;
- `ViewState` — хранит `view`, `phase`, `page`, `cancel_requested`.

Плюс отдельные флаги:

- `discarding`;
- `cancel: Arc<AtomicBool>`;
- `terminal: Option<RecordingTerminal>`;
- `sound_on_completion`;
- `finalizer: Option<JoinHandle<()>>`;
- frontend `cancel_requested`.

Фаза вычисляется из view, но рядом живёт `phase`. Это два источника истины.
Потеря события, двойной `Stop`, `Cancel` и `Lost` не имеют таблицы переходов.

## Целевая модель

### Backend `RecordingMachine`

```rust
enum RecordingState {
    Idle,
    Starting {
        started: Instant,
        page: Page,
        cancel: CancelToken,
    },
    Recording {
        session: RecordingSession,
        started: Instant,
        page: Page,
    },
    Saving {
        started: Instant,
        page: Page,
        recorded: Duration,
        cancel: CancelToken,
    },
    Finished {
        result: SaveResult,
    },
}
```

События:

```rust
enum RecordingEvent {
    StartRequested(RecordingRequest),
    Started(RecordingSession),
    StartFailed(StartFailure),
    StartCancelled,
    PauseToggled,
    StopRequested { action: StopAction, source: StopSource },
    Progress(RecordingProgress),
    SaveFinished(Vec<PathBuf>),
    SaveCancelled,
    SaveFailed(SessionFailure),
    SessionLost,
    Tick(Instant),
}
```

Эффекты:

```rust
enum RecordingEffect {
    StartJob(RecordingRequest),
    StartSession(RecordingRequest),
    StopSession(StopAction),
    SaveFiles(Vec<PathBuf>),
    CancelStart,
    CancelSave,
    Notify(RecordingNotice),
    Sound,
    PublishState,
}
```

### Frontend `RecordingViewState`

Frontend не хранит собственную `phase`; он отображает `RecordingWire` и
хранит только UI-намерение:

```rust
enum RecordingIntent {
    Idle,
    Starting,
    Recording { paused: bool },
    Saving { discarding: bool },
}
```

`ViewState` становится reducer'ом: `apply(view, page)` и производные
`is_recording/is_paused/is_idle` — методы над enum, не отдельные поля.

## Таблица переходов

| Состояние | Событие | Новое | Эффект |
|---|---|---|---|
| Idle | StartRequested | Starting | StartJob |
| Starting | Started | Recording | StartSession, Publish |
| Starting | StartFailed | Idle | Notify, Publish |
| Starting | StartCancelled | Idle | Publish |
| Recording | PauseToggled | Recording | PauseSession, Publish |
| Recording | StopRequested | Saving | StopSession |
| Recording | SessionLost | Idle | Notify Lost, Publish |
| Saving | SaveFinished | Finished | SaveFiles, Sound, Publish |
| Saving | SaveCancelled | Idle | Publish |
| Saving | SaveFailed | Idle | Notify, Publish |
| Finished | Reset | Idle | Publish |

Правила:

- повторный `StopRequested` в `Saving` — no-op;
- `StartRequested` не в `Idle` — `Rejected`;
- `PauseToggled` не в `Recording` — no-op;
- `SaveFinished` после `SaveCancelled` — no-op.

## Что удалить

- `RecordingPhase` как отдельное поле;
- `ViewState.phase`;
- `cancel_requested: bool`;
- `discarding` как отдельный bool — часть `Saving`/`RecordingView`;
- `terminal: Option<...>` — заменить состоянием `Finished`;
- `sound_on_completion` — часть `StartRequested`/`Saving` параметров.

## Тесты (без Windows)

Backend:

1. `start_moves_idle_to_starting`
2. `started_moves_starting_to_recording`
3. `start_failure_returns_to_idle`
4. `stop_from_recording_moves_to_saving`
5. `second_stop_is_ignored`
6. `pause_toggles_only_while_recording`
7. `save_finished_moves_to_finished_once`
8. `cancel_during_saving_returns_to_idle`
9. `lost_session_returns_to_idle_with_notice`
10. `start_requested_while_saving_is_rejected`

Frontend:

1. `view_idle_has_no_phase_field`
2. `view_recording_paused_is_derived`
3. `save_error_clears_recording_intent`
4. `lost_connection_resets_intent`

## Критерии готовности

- В backend и frontend ровно одно состояние записи.
- `RecordingPhase` удалён или является методом.
- Все переходы таблицы покрыты тестами.
- `cancel`/`discard` не являются независимыми bool.
- `cargo test -p mediaa-backend recording` и
  `cargo test -p mediaa-frontend recording` зелёные.
