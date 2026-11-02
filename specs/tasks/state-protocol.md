# ST-9: Protocol / Sync — типизированный контракт вместо строк

## Вопрос

Нужны ли `mediaa-sync` и `mediaa-protocol`?

Да. Но сейчас они используются неправильно: sync умеет версии и события, а
protocol на границе превращается в `String` + `Vec<u8>`. Из-за этого
backend и frontend снова матчат строки и вручную кодируют payload.

## Что есть

### `mediaa-sync` (6 модулей, без доменных зависимостей)

- `Frame` — Hello/Welcome/Subscribe/Snapshot/Changed/Event/Command/Result;
- `StateHub`/`StateView` — версии stores;
- `EventLog` — cursor и gap;
- `Server`/`Client` — переходы.

Это правильный механизм: транспорт, версии, события, ack, resync.

### `mediaa-protocol` (2 модуля, зависит от core/media/platform)

- `backend.rs` — DTO и команды;
- `lib.rs` — константы вида `TRAY_STATE`, `RECORDING_START` и
  `encode_*`/`decode_*`.

Проблема не в наличии двух крейтов, а в том, что контракт **стирает типы**:

```rust
Frame::Command {
    id: u64,
    name: String,       // "tray_state", "recording_start", ...
    payload: Vec<u8>,   // JSON внутри
}
```

Дальше каждый call-site пишет `match name.as_str()` и вызывает
`decode_*`. Это и есть лапша.

## Решение

Оставить два крейта, но разделить ответственность жёстко.

### `mediaa-sync` — только механизм

Не знает про recording/tray/agent. Работает с:

```rust
StoreId
Revision
RequestId
```

и гарантирует:

- handshake;
- snapshot/version;
- event cursor;
- gap -> resync;
- command/result correlation.

### `mediaa-protocol` — только типизированный контракт

```rust
enum StoreId {
    Settings,
    Recording,
    Library,
    Agent,
    Notices,
}

enum ClientCommand {
    Tray(TrayCommand),
    Recording(RecordingCommand),
    Agent(AgentOp),
    Preview(PreviewRequest),
    Settings(Settings),
}

enum ServerEvent {
    Tray(TrayCommand),
    Recording(RecordingEvent),
    Agent(AgentEvent),
    Notice(Notice),
}
```

Никаких `pub const TRAY_STATE: &str`, `encode_tray_update`,
`decode_tray_command` в публичном API. Кодирование — внутри protocol,
тип — снаружи.

### Backend/frontend

- backend: `match ClientCommand` по enum;
- frontend: `match ServerEvent` по enum;
- один reducer на store;
- строки остаются только в JSON-теге serde.

## Что удалить

- строковые константы команд/событий;
- публичные `Vec<u8>` payload;
- `encode_*`/`decode_*` на каждый домен;
- `pending_agent_sends: HashSet<u64>` — заменить `RequestId` + состояние
  запроса.

## Тесты

Без Windows:

1. `client_command_roundtrips_through_json`
2. `server_event_roundtrips_through_json`
3. `unknown_command_is_rejected_with_typed_error`
4. `store_snapshot_versions_are_monotonic`
5. `event_gap_requests_resync`
6. `command_result_matches_request_id`
7. `old_event_is_ignored`
8. `frontend_reducer_applies_snapshot_then_event`

## Критерии готовности

- В `mediaa-sync` нет доменных строк.
- В `mediaa-protocol` нет публичных строковых команд.
- Backend/frontend не содержат `match "tray_state"`.
- `Frame::Command` не несёт `name: String`.
- `cargo test -p mediaa-sync` и `cargo test -p mediaa-protocol` зелёные.

## Нужно ли сливать крейты?

Нет, пока контракт типизирован. Слияние в один `mediaa-ipc` уменьшит число
пакетов, но не уберёт главную проблему — строковую границу. Если позже
нужно, сливать можно как `sync::transport` + `sync::contract`, но это
косметика, а не решение.
