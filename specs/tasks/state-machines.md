# ST: состояние приложения как явные state machines

## Статус

`в работе`: аудит и задачи. Реализация идёт по доменам, без полного прогона
тестов и без релизных сборок.

## Зачем

Сейчас одно состояние хранится в нескольких местах:

- `Option<Connection>` + `bool` + `Option<Instant>` в backend;
- `hidden` + `startup` + `quit_requested` + `idle_since` во frontend;
- отдельные `phase`/`view`/`status` в recording и conversion;
- кэши вроде `tray_applied`, `applied`, `conflict_notified`.

Из-за этого переходы не проверяются, а баги лечатся новыми флагами. Пример
трея: первый UI поднимается и сам завершается, второй клик поднимает
следующий. Это не «клик не дошёл», это несогласованный lifecycle.

Правило: **состояние — это enum, переход — это событие, результат перехода —
эффект.** Всё остальное — производные данные.

## Общие правила

1. Один владелец состояния на процесс/слой.
2. Никаких `bool`-флагов, кодирующих фазу (`show_on_start`,
   `show_ack_pending`, `probe_loading`, `audio_waiting` и т.п.).
3. Время передаётся в переход, а не читается внутри:
   `on(event, now)`, `tick(now)`.
4. Переход возвращает `Vec<Effect>`, а не сам выполняет I/O.
5. Невалидный переход — явный no-op или `Rejected`, и он тестируется.
6. Тесты переходов не требуют Windows, окна, трея и `sleep`.
7. Windows-smoke — только после зелёных unit-тестов, отдельной командой.
8. Один домен = один task-документ и один набор тестов.

## Инвентаризация

| Домен | Где сейчас | Главные дыры | Задача |
|---|---|---|---|
| App / окно / трей / выход | `backend/daemon`, `frontend/app`, `window_presence` | `show_on_start`, `quit_when_idle`, `quit_requested`, `idle_since`, `show_*`, первый UI умирает | [state-app-lifecycle.md](state-app-lifecycle.md) |
| Recording | `features/recording` backend/frontend | два `RecState`, `phase` выводится из view, `discarding`, `cancel_requested`, `terminal` | [state-recording.md](state-recording.md) |
| Capture / overlay | `frontend/app/capture.rs`, `ui/region_editor.rs` | `after_completion`, `restore`, `hide`, `Option<Overlay>` | [state-capture.md](state-capture.md) |
| Conversion / batch | backend/frontend `features/conversion` | `job`, `batch_auto`, `probe_loading`, `conflict_notified`, `failed`, `run_started` | [state-conversion.md](state-conversion.md) |
| Library / player | `features/library`, `player.rs`, `view.rs` | `paused`, `got_frame`, `audio_waiting`, `audio_needs_alignment`, `dragging`, generations | [state-library.md](state-library.md) |
| Agent UI | `ui/pages/agent`, `live_buffer`, `requests` | глобальные `Option`, `live` map + snapshot + state, очередь запросов | [state-agent.md](state-agent.md) |
| Backend services / IPC | `settings_file`, `preview`, `clipboard`, `recovery`, `sync` | `stopping`/`finished`, `pending`, event cursor, pending events | [state-services.md](state-services.md) |
| App shell / god object | `frontend/app.rs` | 45 полей, 858 строк, смешаны 12 доменов | [state-app-shell.md](state-app-shell.md) |
| Protocol / sync | `mediaa-sync`, `mediaa-protocol` | типы стираются в `String` + `Vec<u8>` | [state-protocol.md](state-protocol.md) |
| Ownership | protocol / backend / frontend | неясно, кто выполняет переходы | [state-ownership.md](state-ownership.md) |
| Test strategy | все домены | нельзя смешивать pure-переходы и ОС | [state-test-strategy.md](state-test-strategy.md) |
| Frontend-owned state | `Settings.viewer_volume`, UI prefs | клиентское состояние сохраняется backend-ом | [state-client-state.md](state-client-state.md) |

## Порядок работ

Задачи созданы все сразу. Код идёт волнами, чтобы не смешивать домены:

1. **Волна A — чистые модели без I/O:**
   `state-app-lifecycle`, `state-recording`, `state-capture`.
2. **Волна B — данные и фоновые контроллеры:**
   `state-conversion`, `state-library`, `state-agent`.
3. **Волна C — инфраструктура:**
   `state-services`.
4. После каждой волны — только её targeted-тесты.

Каждая задача обязана:

- добавить enum состояний/событий/эффектов;
- удалить старые флаги этого домена;
- добавить таблицу переходов в task-документ;
- покрыть все переходы unit-тестами;
- не менять другие домены.

## Что считается готовым

- В домене нет флагов, кодирующих фазу.
- Все переходы таблицы покрыты тестами.
- Тесты не требуют Windows/экрана/трея.
- Для каждого перехода есть ожидаемый эффект.
- `cargo test -p <crate> <module>` зелёный.
- Только после этого — один ручной smoke, если домен связан с ОС.

## Запрещено

- Лечить баг новым `bool`.
- Добавлять `sleep` в тесты переходов.
- Запускать полный `cargo test` и релизную сборку на каждом шаге.
- Смешивать рефакторинг двух доменов в одном изменении.
- Оставлять неиспользуемые старые поля «на всякий случай».
