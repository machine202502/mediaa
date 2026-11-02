# Единая синхронизация демон ↔ UI

Статус: миграция домена завершена (SYNC-1…9b); остались SYNC-10 — финальная документация и замеры. Цель — один механизм вместо набора частных договорённостей в `adapters/ipc.rs`.

## Решение

Два крейта:

- `crates/sync` (`mediaa-sync`) — движок: кадры, кодек, версии состояния, команды, события,
  resync, heartbeat. Не знает про MediaA.
- `crates/protocol` (`mediaa-protocol`) — весь контракт: `StoreId`, payload-типы, `Command`,
  `Event`, версия протокола. Зависит от DTO `core`/`storage`/`platform`, но не от `mediaa` (app).

Приложение реализует серверные хендлеры и клиентские биндинги, не изобретая синхронизацию
заново в каждом месте.

## Транспорт и кодек

- Транспорт остаётся `interprocess` (named pipe).
- Кадры — `u32` длина + payload. Никакого `\n`-фрейминга.
- Кодек за трейтом `Codec`: сейчас JSON для отладки, позже бинарный (`postcard`/MessagePack)
  без смены конверта. Payload домена кодируется отдельно protocol-крейтом.
- Без Tokio: std threads + `crossbeam-channel`.

## Конверт

```text
Hello      { version, client }
Welcome    { version, session, event_seq, metadata }
Subscribe  { stores, versions, event_seq }
Snapshot   { store, version, payload }
Changed    { store, version, payload }
Resync     { stores, event_seq }
Command    { id, name, payload }
Result     { id, ok, payload, error }
Event      { seq, kind, payload }
Ping / Pong
Reject     { reason }
Error      { code, message }
```

Правила:

- `Snapshot` — полное состояние store; приходит на `Subscribe` и `Resync`.
- `Changed` — только если версия больше последней отправленной клиенту; coalescing: несколько
  изменений до отправки схлопываются в последнее.
- клиент применяет только версию больше известной; иначе игнорирует.
- `Resync` — если клиент видит разрыв версий/событий или переподключился.
- `Command` — id, ответ `Result` с тем же id; таймаут; для неидемпотентных команд сервер
  кэширует результат по id и повторно не выполняет.
- `Event` — seq; клиент хранит курсор; при пропуске — `Resync`.
- соединение: `Hello/Welcome` проверяет версию; при обрыве клиент переподключается и
  подписывается заново со своими версиями; сервер отвечает `Snapshot`/`Changed`/событиями.

## Кто что хранит

- Демон — единственный писатель состояния. `StateHub` хранит `(version, payload)` по store.
- UI — только читатель состояния и отправитель команд. `StateView` хранит известные версии.
- События — ring в демоне (`EventLog`), курсор в UI.
- Команды — сервер выполняет и отвечает; UI не меняет состояние сам, пока не придёт новый
  снапшот/событие.

## Тестовая матрица (обязательная)

Состояние:

- подписка → снимки всех stores;
- повторный `Changed` не отправляется, если версия не выросла;
- несколько `publish` до `flush` → один `Changed` с последней версией;
- `version` меньше известной → игнор;
- потеря `Changed` → `Resync` возвращает актуальный снимок;
- переподключение с версиями → `Snapshot`/`Changed` без потери финального состояния;
- запись переходит `Saving → Idle` → UI получает Idle (наш FIX-2).

Команды:

- `Result` приходит с тем же id;
- таймаут без ответа;
- повтор с тем же id не выполняет команду дважды (кэш результата);
- команда до `Subscribe`;
- неизвестная команда → `Error`/`Result` с ошибкой, соединение живо;
- две команды подряд → порядок ответов не ломает состояние.

События:

- seq монотонен;
- `since(cursor)` отдаёт пропущенное;
- вытеснение из ring → gap/resync;
- дубликат события игнорируется по seq;
- события до подписки не теряются (ring).

Соединение:

- несовпадение версии → понятный `Error` и закрытие;
- обрыв во время команды;
- обрыв во время записи → переподключение видит актуальное состояние;
- heartbeat: молчащий peer определяется;
- oversize/malformed кадр → ошибка, без паники.

## Подзадачи

| ID | Задача | Критерий |
|---|---|---|
| SYNC-1 | Крейты `mediaa-sync`/`mediaa-protocol`, кадры и кодек | length-prefix, roundtrip, версия |
| SYNC-2 | `StateHub`, `StateView`, `EventLog` | версии/coalescing/ring, unit-тесты |
| SYNC-3 | `Server`/`Client` state machines | subscribe/snapshot/changed/resync/command/event |
| SYNC-4a | Settings: состояние | стартовый снимок и изменения через store |
| SYNC-4b | Settings: команды | SetSettings с ack вместо SettingsSync |
| SYNC-5a | Recording: состояние | store `recording`, version на каждом изменении, UI применяет |
| SYNC-5b | Recording: команды | start/stop/pause/cancel через Command + ack |
| SYNC-5c | Recording: события | finished/window/files через EventLog |
| SYNC-6 | Library | версия/поколение, reload-only |
| SYNC-7 | Notices, recovery | события + ring |
| SYNC-8 | Tray, hotkeys, picker | события/команды |
| SYNC-9a | Убрать доменные `Message`; picker — командой | один протокол для домена |
| SYNC-9b | Handshake/launcher на `Frame` | удалить `Message`/`Incoming` |
| SYNC-9 | Удалить старый `Message`/`wire.rs` | один протокол |
| SYNC-10 | Документация и замеры | architecture/constraints/progress | готово |

Порядок: SYNC-1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10. Каждый шаг заканчивается тестами и
зелёным `cargo test`; старый протокол живёт, пока домен не мигрирован.

## Журнал

- 02.11.26, SYNC-3: `Server`/`Client` state machines. Подписка, снапшоты, `Changed` только
  при росте версии, coalescing, `Resync`, кэш результатов команд, события по seq, gap.
  25 тестов в `mediaa-sync`.
- 02.11.26, транспорт: `ipc::Connection` переведён на length-prefixed кадры; старые сообщения
  идут как `Frame::Extension { channel: "legacy" }`, поэтому миграция доменов возможна по одному.
- 02.11.26, SYNC-9b: handshake на `Frame`. Acceptor читает `Hello { version, client }`,
  классифицирует `ui`/`launcher`, отвечает `Welcome { metadata: { tray } }` или `Reject`.
  UI получает `Welcome` и создаёт `Client` без второго hello; лаунчер после welcome шлёт
  `picker_starting` командой. `Message`, `Incoming`, `client_role`, `Extension`, старые
  `hello/welcome/reject` удалены — остался только `Frame`.
- 02.11.26, SYNC-9a: лаунчер объявляет пикер sync-командой `picker_starting`, а не
  `Message::PickerStarting`. Все доменные `Message`-варианты удалены; остались только
  `Hello`/`Welcome`/`Reject` транспортного handshake. Смоук `--area`: маска/пикер
  открываются.
- 02.11.26, SYNC-8: tray/hotkeys/picker. `TrayCommand` и `Hotkeys` — события
  (`TRAY_EVENT`/`HOTKEYS_EVENT`), `HotkeysPaused`, `TrayState`, `Quit`, `PickerShown` — команды.
  Если UI ещё не подключён, события ждут в `pending_events` и вливаются в EventLog сразу после
  `Welcome` — иначе клиент пропустил бы их по курсору. Старые `TrayCommand`/`Hotkeys`/
  `HotkeysPaused`/`TrayState`/`Quit`/`PickerShown` сообщения удалены; `PickerStarting` пока
  остаётся для лаунчера.
- 02.11.26, SYNC-7: notices и recovery. Заметки — store `notices` (ring с id): UI
  добавляет только записи с id больше последнего, поэтому повторный snapshot не дублирует.
  Recovery — store `recovery { running }` + событие `RecoveryReport` через EventLog; UI
  блокирует запись по running и показывает отчёт по событию. Старые `Notices`/`Notice`/
  `Recovery` сообщения удалены.
- 02.11.26, SYNC-6: library — store `library`. Демон публикует `LibraryChangeWire`
  (paths/rescan) с версией после watcher-а и загрузки индекса; UI по `State`-событию
  перезагружает индекс только для чтения. `Message::LibraryChanged` удалён. Загрузки
  коалесцируются: если `reload` пришёл во время загрузки, после неё запускается ещё одна.
- 02.11.26, SYNC-5b/5c: команды и события записи. `start/stop/toggle/cancel` идут
  `Command`-ами с id/ack, `RecordingEventWire` — через `EventLog` с seq. Старые
  `RecordingCommand`/`RecordingEvent` сообщения удалены. `Welcome` теперь несёт `event_seq`,
  чтобы новый клиент не принял первый живой event за gap и не потерял `Finished`; UI просит
  repaint на каждое сообщение из канала. Смоук: `Finished` доехал, файл попал в clipboard.
- 02.11.26, SYNC-5a: состояние записи — в store. `RecordingView`/`RecordingInfo`/`Page`
  преобразуются в `RecordingStateWire` в `wire.rs`; демон публикует `RECORDING` с новой версией,
  UI подписан на `settings`+`recording` и применяет только новые версии. Финальный `Idle`
  теперь тоже уходит версией — FIX-2 закреплён механикой. Команды и события пока старые.
- 02.11.26, SYNC-4b: `SetSettings` — команда с ack. `Server` больше не владеет handler-ом:
  `on_frame` возвращает `Command { id, name, payload }`, демон выполняет и вызывает
  `complete_command(id, result)`. UI шлёт `SetSettings`, получает `Result`, а состояние
  приходит отдельным `Changed`. Старые `SettingsSync`/`SettingsRequest`/`SettingsSnapshot`
  удалены. Тесты: команда/повтор id, handshake, потеря Changed, переподключение.
- 02.11.26, SYNC-4a: settings — первый store. UI при старте делает sync-handshake
  (`Hello`/`Welcome`/`Subscribe`) и получает `Snapshot`; старые `TrayCommand` буферизуются до
  снимка. Демон публикует новую версию при `SettingsSync` и шлёт `Changed`. Проверено смоуком:
  UI стартует, tray-пикер открывается. Команда `SetSettings` — следующий шаг.
## Итог

Тесты на 02.11.26:

- `mediaa-sync`: 28 (state/version/coalescing/resync/command dedupe/gap/reconnect/event cursor/ping);
- `mediaa-protocol`: 8 roundtrip-тестов контракта;
- `mediaa-app`: 414;
- `adapters::ipc`: 3 (framing, clone, EOF);
- architecture gates: без нарушений.

Замеры (debug, Windows 11, изолированный экземпляр):

- демон idle: 18.3 MB RSS, 0.3 % одного ядра (включая индекс библиотеки и watcher);
- UI settings на GL (`WGPU_BACKEND=gl`): 328.5 MB, 0 %;
- UI settings на Vulkan: 591.4 MB, 99 % — известное поведение eframe, не sync;
- запись GIF 320×200: событие `Finished` доходит, файл попадает в clipboard;
- `--area` и tray-команда открывают пикер через события.

Смоук-проверки выполнялись только проектными скриптами, окна скрыты/за экраном.