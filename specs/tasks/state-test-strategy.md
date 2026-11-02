# ST-11: Как проверяем состояния и что не проверяется

## Короткий ответ

Да, все **переходы** можно проверить без Windows. Нет, нельзя unit-тестами
проверить реальное окно, трей, запись экрана, ffmpeg и keychain. Их надо
изолировать за портами и проверять отдельно.

## Четыре уровня проверки

### 1. Чистые переходы

Один reducer на домен:

```rust
fn apply(state: State, event: Event, now: Instant) -> (State, Vec<Effect>)
```

Проверяем:

- каждую пару `state × event` из таблицы;
- невалидные переходы;
- повторные события;
- устаревшие события;
- таймауты с фейковым временем;
- идемпотентность.

Без окон, потоков, файлов, `sleep`.

### 2. Исполнение эффектов

Backend-эффекты проверяются с фейковым исполнителем:

```rust
trait TrayEffects {
    fn spawn_visible(&mut self);
    fn send_show(&mut self);
    fn restart_visible(&mut self);
    fn quit(&mut self);
}
```

Тест проверяет не «процесс запустился», а «machine вернула
`SpawnVisible`, executor получил ровно один вызов».

### 3. Protocol / sync

Проверяем:

- typed command/event roundtrip;
- версии snapshot;
- `Event(seq)` в порядке;
- старый event игнорируется;
- gap даёт `Resync`;
- command/result correlation.

Без IPC и без Windows.

### 4. Frontend projection

Frontend применяет те же переходы к своей проекции:

```rust
fn project(state: BackendState, event: BackendEvent) -> UiState
```

Проверяем, что UI не придумывает состояние и что optimistic-состояния
исчезают при отказе.

### 5. Frontend-owned persistence

Клиентские предпочтения не идут через backend. Проверяем отдельно:

1. `volume_survives_restart`
2. `unknown_field_is_ignored`
3. `corrupt_file_falls_back_to_defaults`
4. `old_schema_is_migrated`
5. `write_is_atomic`
6. `backend_never_receives_client_preferences`

Файл принадлежит frontend-у, версия схемы обязательна.


## Что нельзя проверить unit-тестами

- реальное появление окна на экране;
- фокус Windows;
- клик по нативному меню трея;
- поведение `tray-icon`/`winit`;
- реальную запись экрана/микрофона;
- ffmpeg-кодеки;
- keychain;
- гонки драйверов GPU.

Для них — отдельный smoke на Windows. Unit-тесты гарантируют **логику
переходов**, а не поведение ОС.

## Подводные камни

### 1. Состояние сейчас размазано между процессами

Backend и frontend имеют разные копии. Пока нет авторитетного владельца,
«все переходы» проверить нельзя: непонятно, чей переход истинный.

**Решение:** backend — авторитет; frontend — проекция.

### 2. `Instant::now()` внутри переходов

Тест станет недетерминированным.

**Решение:** время только параметром `now`; таймеры — `deadline` в
состоянии.

### 3. `bool`-флаги не образуют состояние

`show_on_start`, `quit_requested`, `audio_waiting`, `probe_loading` нельзя
перебрать как состояния.

**Решение:** enum + таблица переходов; флаги удалить.

### 4. Эффекты — это I/O

Нельзя в unit-тесте запускать процесс или писать файл.

**Решение:** эффект — enum; исполнитель — отдельный trait; в тестах —
fake.

### 5. Глобальные `static`/`Mutex`

`live_buffer`, i18n, single instance мешают параллельным тестам.

**Решение:** состояние — поле контроллера; глобальные тестовые reset не
нужны.

### 6. Реальные jobs и каналы

`RecordingController`/`CaptureController` завязаны на `Job`, `Receiver`,
`RecordingSession`.

**Решение:** выделить pure state machine, а jobs/каналы передавать как
эффекты.

### 7. Старые события и гонки

Событие от старого поколения может прийти после нового старта.

**Решение:** `generation`/`RequestId`; stale — no-op, и это тестируется.

#### Что такое stale event

Stale event — событие, которое относится к прошлой жизни состояния, но
пришло позже нового. Примеры:

- запись A отменили, начали запись B, а поздно пришёл `Finished(A)`;
- окно закрыли, открыли заново, а поздно пришёл `WindowShown` от старого
  UI-процесса;
- агент перезапустили, а поздно пришёл `TaskDone` от старого хоста;
- UI переподключился, а в очереди лежит `Event(seq=3)` от прошлой сессии.

Это не дубликат и не gap. Это событие **из другого поколения**.

Механизм:

```text
generation: u64   // растёт при reset/restart/reconnect
Event { generation, seq, ... }
```

Reducer:

- `event.generation < state.generation` -> ignore;
- `event.generation > state.generation` -> `Resync`;
- равное поколение: `seq <= cursor` -> ignore; `seq == cursor+1` -> apply;
  иначе gap -> `Resync`.

`RequestId` работает так же для команд: ответ на старый request не меняет
новое состояние.

Тесты:

1. `old_generation_window_shown_is_ignored`
2. `old_recording_finished_does_not_finish_new_recording`
3. `old_agent_event_is_ignored_after_reset`
4. `same_generation_old_seq_is_ignored`
5. `same_generation_gap_requests_resync`
6. `response_to_old_request_is_ignored`


### 8. Ошибки протокола

Сейчас `Frame::Command { name: String, payload: Vec<u8> }` не даёт
типизировать неизвестную команду.

**Решение:** typed enum + `Rejected(error)`.

### 9. Backend/frontend разные UI-состояния

Не всё состояние — доменное. Вкладки, hover, scroll не должны ехать через
sync.

**Решение:** `DomainState` (backend) и `UiState` (frontend) раздельно.

### 10. Полный перебор комбинаций

`state × event × payload` может взорваться.

**Решение:** тестировать таблицу переходов + инварианты, а не все
комбинации данных. Для сложных — property-based тесты.

## Что должно быть решено до старта

1. Где живёт общий reducer: `mediaa-protocol::state` или `mediaa-state`.
2. Кто авторитетен в каждом домене: backend или frontend.
3. Что делать с невалидным переходом: `Rejected`, no-op или panic.
4. Как передавать время: `now`/`Clock`.
5. Какой trait у эффектов каждого домена.
6. Какие события считаются stale и как это определяется.
7. Какие поля — доменные, а какие — локальные UI.
8. Нужна ли обратная совместимость protocol (сейчас — нет, один app).

## Критерии «переходы проверены»

- Для каждого домена есть таблица `state × event`.
- Каждая строка таблицы имеет тест.
- Каждый невалидный переход имеет тест.
- Каждый эффект проверен fake-executor’ом.
- Frontend projection проверена теми же событиями.
- Sync проверен на порядок/дыры/дубли.
- Windows-smoke не входит в обычный `cargo test`.

## Что я не буду делать

- Называть «проверено», если тестировался только happy path.
- Запускать полный suite и релиз на каждом шаге.
- Добавлять флаг, если перехода нет в таблице.
- Оставлять «временный» `bool` без задачи.
