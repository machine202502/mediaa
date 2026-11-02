# BUG: трей не открывает окно и медленно завершает приложение

## Симптомы

1. После закрытия окна в трей пункт «Открыть MediaA» не открывает окно.
2. «Выход» из трея завершает приложение примерно через 5 секунд.

## Причины

### Потерянное событие при повторном запуске UI

Когда UI-процесс уже завершился, трей-команда ставится в
`Daemon::pending_events`, а затем backend поднимает UI в трее. Событие
отправлялось сразу после `Welcome`, **до** `Subscribe` от UI. Сервер
sync трактует `event_seq == 0` как «начать с последнего события» и
пропускал уже поставленное событие. В результате UI стартовал скрытым и
никогда не получал `ShowWindow`.

Исправление: `flush_pending_events` перенесён из `handle_accept` в
обработку `Frame::Subscribe`. Событие теперь ставится после того, как UI
подписался и его курсор событий зафиксирован. Если UI-процесса нет,
запускается **видимый** экземпляр с аргументом `--show`, а не скрытый
`--tray` (`ensure_ui_visible`, новый `StartupAction::Show`). Это важно,
даже если в настройках включён `start_in_tray`.

Для случая «первый клик теряется, второй работает» добавлен ack:
UI после `ShowWindow` отправляет `TRAY_SHOW_ACK`. Если backend не получил
ack за 700 мс, он считает скрытый UI неответившим, отбрасывает его
соединение и запускает видимый `--show` (`show_ack_pending`,
`show_retry_at`, `poll_show_retry`). Это гарантирует открытие с первого
клика, даже если событие потерялось в скрытом UI.

### Долгий выход

`SettingsWriter` при каждом сохранении настроек вызывал
`mediaa_agent::secrets::save`, то есть обращение к OS keychain. На выходе
`Daemon::quit` ждал `SettingsWriter::flush` до 2 секунд, а keychain мог
добавить ещё несколько секунд. Секреты уже сохраняются в
`apply_settings` только при фактическом изменении, поэтому повторное
сохранение из writer-а убрано. Теперь при простое выход не ждёт keychain.

Дополнительно: `Quit` при `recording_view.is_idle()` и
`!conversion.is_busy()` завершает daemon сразу, не отправляя команду в UI
и не дожидаясь его idle-таймера. Если идёт запись/конвертация, команда
по-прежнему уходит UI для корректного завершения.

## Файлы

- `crates/backend/src/daemon/dispatch.rs` — `flush_pending_events` после
  `Subscribe`, `ensure_ui_visible`.
- `crates/backend/src/daemon/mod.rs` — быстрый `Quit`, `ensure_ui_visible`.
- `crates/backend/src/infra/settings_file.rs` — writer больше не трогает
  keychain.
- `crates/backend/src/infra/tray.rs` — `refresh` для смены языка.
- `crates/backend/src/daemon/services.rs` — `MEDIAA_DEV_TRAY` для всех
  tray-команд, ack/повтор `ShowWindow`.
- `scripts/windows/checks/tray-action.ps1` — запуск debug-копии агента с
  виртуальной tray-командой.
- `crates/frontend/src/app.rs` — `StartupAction::Show`, `--show`.
- `crates/protocol/src/lib.rs` — `TRAY_SHOW_ACK`.
- `crates/frontend/src/session.rs` — `--show` не уходит в tray.
- `crates/frontend/src/app/actions.rs` — обработка `StartupAction::Show`.

## Проверка

- `cargo test -p mediaa-backend --lib` — все тесты.
- `cargo check --workspace --all-targets`.
- Ручной сценарий: закрыть окно в трей, подождать >3 с (UI-процесс
  завершится), нажать «Открыть MediaA» — окно должно появиться.
- Ручной сценарий: закрыть в трей, нажать «Выход» — процесс должен
  завершиться сразу, без пятисекундной паузы.
- Виртуальная проверка:
  `.\scripts\windows\checks\tray-action.ps1 -Action quit -WaitSec 6`
  (debug-копия агента, изолированный instance).
