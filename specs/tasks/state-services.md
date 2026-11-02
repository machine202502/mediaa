# ST-7: Backend services / IPC — явные worker-состояния

## Где

- `crates/backend/src/infra/settings_file.rs`
- `crates/backend/src/features/clipboard/mod.rs`
- `crates/backend/src/features/preview/store.rs`
- `crates/backend/src/features/library/mod.rs`
- `crates/backend/src/daemon/notices.rs`
- `crates/sync/src/server.rs`
- `crates/sync/src/client.rs`

## Текущее состояние

### SettingsWriter

`WriterState` одновременно:

- `pending: Option<SaveRequest>`
- `written: u64`
- `failure: Option<(u64, String)>`
- `stopping: bool`
- `finished: bool`

`stopping` и `finished` — две фазы одного жизненного цикла. `pending`
может быть `Some` после `stopping`.

### Clipboard

`ClipboardHandle`:

- `in_flight: AtomicUsize`
- `available: AtomicBool`
- канал команд с `reply: Option<Sender>`

`available` и `in_flight` не образуют проверяемое состояние; timeout
оставляет счётчик в неопределённости.

### Preview

`PreviewStore` хранит строки БД `state = queued|generating|ready|failed`,
но рядом есть `PreviewDecision::Pending { enqueue: bool }`.
Состояние очереди размазано между БД и решением.

### Sync

`Server`:

- `handshaken: bool`
- `subscribed: BTreeSet`
- `sent_versions: BTreeMap`
- `event_cursor: u64`
- `results: HashMap`
- `events: EventLog`

`Client`:

- `event_cursor`, `pending: BTreeSet`, `handshaken`, `session`.

Переход `Hello -> Subscribe -> Event` не имеет тестов на все ошибки:
повторный Hello, Subscribe до Hello, Event с дырой, Resync.

## Целевая модель

### SettingsWriter

```rust
enum WriterState {
    Idle,
    Saving { generation: u64, request: SaveRequest },
    Stopping { generation: u64 },
    Finished,
}
```

### Clipboard

```rust
enum ClipboardState {
    Available,
    Busy { in_flight: usize },
    Unavailable { reason: String },
}
```

### Preview

```rust
enum PreviewState {
    Missing,
    Queued { requested_at: u64 },
    Generating,
    Ready { path: PathBuf },
    Failed { error: String },
}
```

### Sync

```rust
enum SyncPhase {
    New,
    Handshaken { session: u64 },
    Subscribed { stores: BTreeSet<String> },
    Closed,
}
```

`event_cursor` — данные, но переходы `Event(seq)` проверяются:
`seq == cursor+1` -> apply; `seq <= cursor` -> ignore; иначе -> Gap/Resync.

## Тесты (без Windows)

1. `settings_save_then_stop_finishes_once`
2. `settings_stop_with_no_pending_finishes`
3. `clipboard_busy_then_available`
4. `clipboard_disconnect_marks_unavailable`
5. `preview_queued_then_ready`
6. `preview_failed_then_requeue`
7. `sync_rejects_subscribe_before_hello`
8. `sync_duplicate_hello_resets_session`
9. `sync_event_gap_requests_resync`
10. `sync_old_event_is_ignored`

## Критерии готовности

- Нет пар `stopping/finished`, `available/in_flight` как фазы.
- `PreviewDecision` заменён состоянием.
- Sync-переходы покрыты тестами с `Server`/`Client` без IPC.
- `cargo test -p mediaa-backend settings` и
  `cargo test -p mediaa-sync` зелёные.
