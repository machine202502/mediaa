# Удалённые UI-тесты агента

Здесь лежат тест-модули, удалённые из frontend при переходе агентского UI
на protocol и удалении `AgentView`/`AgentServices`.

Источник:

```text
commit d4401ff49d33e483bd05097a31546b9a30f4472c
crates/app/src/ui/pages/agent/queue.rs
crates/app/src/ui/pages/agent/analysis.rs
crates/app/src/ui/pages/agent/task_details.rs
crates/app/src/ui/pages/agent/dialog.rs
crates/app/src/ui/pages/agent/chat.rs
```

В каждом файле лежит только блок `#[cfg(test)] mod tests { ... }`,
извлечённый из указанной версии. Код тестов не адаптирован: он ссылается
на старые `AgentServices`, `AgentView`, `mediaa_queue::Task`,
`mediaa_agent::pipeline` и старые сигнатуры. Поэтому файлы нельзя просто
скопировать в `crates/frontend/src` — их нужно портировать по плану
[restore-agent-ui-tests.md](../restore-agent-ui-tests.md).

## Файлы

| Файл | Старый модуль | Тестов |
|---|---|---|
| [queue.rs](queue.rs) | `ui/pages/agent/queue.rs` | 13 |
| [analysis.rs](analysis.rs) | `ui/pages/agent/analysis.rs` | 15 |
| [task_details.rs](task_details.rs) | `ui/pages/agent/task_details.rs` | 13 |
| [dialog.rs](dialog.rs) | `ui/pages/agent/dialog.rs` | 5 |
| [chat.rs](chat.rs) | `ui/pages/agent/chat.rs` | 8 |

Итого: **54 теста**.

## Почему они не компилируются как есть

- `queue`/`analysis`/`task_details` используют `mediaa_queue::{Task, TaskId,
  Stats, Status}`; теперь frontend использует `features::agent::view::Task`
  и `AgentTaskViewWire`.
- `dialog`/`chat` используют `AgentServices`, `AgentView`, `Scheduler`,
  `Store`, `Live` из `mediaa_agent`.
- `read(store, filter)` заменён на `read(&AgentSnapshotWire, &Filter)`.
- `Task` теперь строится из `AgentTaskViewWire`, а не из `MemStore`.
- `chat`/`dialog` работают через `live_buffer`, `requests` и
  `AgentCommand`, а не через прямой `AgentServices`.

## Как пользоваться

1. Открыть [restore-agent-ui-tests.md](../restore-agent-ui-tests.md).
2. Взять тест из этого каталога как эталон ожидаемого поведения.
3. Портировать фикстуры на wire-типы.
4. Не возвращать `mediaa_queue`/`mediaa_agent` в зависимости frontend.
