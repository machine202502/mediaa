# ST-6: Agent UI — один reducer вместо глобального LiveBuffer

## Где

- `crates/frontend/src/ui/pages/agent/live_buffer.rs`
- `crates/frontend/src/ui/pages/agent/chat.rs`
- `crates/frontend/src/features/agent/requests.rs`
- `crates/frontend/src/features/agent/view.rs`
- `crates/backend/src/features/agent/requests.rs`
- `crates/backend/src/features/agent/host.rs`

## Текущее состояние

Frontend:

- глобальный `LIVE: Mutex<Option<LiveBuffer>>`;
- внутри одновременно `live: HashMap<i64, Live>`, `state: Option<AgentStateWire>`,
  `snapshot: Option<AgentSnapshotWire>`;
- `requests` хранит ожидание ответов, `pending_agent_sends: HashSet<u64>`;
- UI-агент `AgentUi` дополнительно хранит вкладки/очередь.

Backend:

- `AgentRequests { jobs: Vec<Pending> }`;
- `AgentHost` с `connections: Option`, `applied: Vec`, `hosted: Option`,
  `checked: Option<PathBuf>`, `pending: bool`;
- `AgentServices` собирается из хоста.

Состояние «агент запущен», «модель загружается», «запрос в полёте»,
«ответ пришёл», «соединение потеряно» размазано между `hosted`, `pending`,
`state`, `snapshot` и live-map. Глобальный `Mutex` не даёт нормально
тестировать переходы.

## Целевая модель

```rust
enum AgentState {
    Disabled,
    Starting { connection: String },
    Ready { models: Vec<String> },
    Working { task: i64, progress: Option<f32> },
    Unavailable { reason: String },
}
```

Frontend reducer:

```rust
enum AgentEvent {
    Snapshot(AgentSnapshotWire),
    State(AgentStateWire),
    Live(AgentEventWire),
    Response(AgentResponseWire),
    RequestSent(u64),
    RequestFailed(u64, String),
    Reset,
}
```

`AgentUiState` — обычная структура без `static Mutex`; владелец — `App`.
Тесты гоняют события и проверяют:

- задачи/стримы;
- `live` очищается по `task_done`;
- ответ на неизвестный request игнорируется;
- snapshot заменяет state, а не дополняет;
- `Reset` очищает всё.

Backend host:

```rust
enum HostState {
    Stopped,
    Starting,
    Hosted { connections: Arc<Connections> },
    Restarting { reason: String },
}
```

`pending` заменяется фазой `Starting`/`Restarting`; `checked` — это кэш
файла, не состояние.

## Тесты (без Windows, без сети)

1. `snapshot_replaces_previous_snapshot`
2. `live_event_for_unknown_task_is_ignored`
3. `task_done_removes_live_stream`
4. `response_for_unknown_request_is_ignored`
5. `request_sent_then_failed_clears_pending`
6. `reset_clears_snapshot_live_and_requests`
7. `host_start_failure_moves_to_unavailable`
8. `host_restart_clears_applied_connections`

## Критерии готовности

- Нет глобального `LIVE`/`Mutex<Option<...>>`.
- `snapshot`, `state`, `live` не живут как три независимых `Option`.
- Request map имеет явные состояния `Pending/Done/Failed`.
- Все переходы покрыты тестами.
- `cargo test -p mediaa-frontend agent` и
  `cargo test -p mediaa-backend agent` зелёные.
