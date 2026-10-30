# Действия трея не меняют состояние окна

## Решение

- Единственное действие трея, которое показывает окно, — явные «Открыть окно»/левый клик с
  настройкой «Открыть окно». Остальные не скрывают, не показывают и не поднимают окно.
- Запись экрана и аудио из трея передаёт `RecordingCommand::Start { hide: Some(false) }` и
  ставит `recording_quiet = true`. Все ветки завершения/ошибок записи проверяют
  `recording_quiet` и не вызывают `show_window`.
- Остановка/отмена из трея тоже ставит `recording_quiet = true`, поэтому завершение не поднимает
  окно. UI-остановка сбрасывает флаг.
- Снимок области из трея запускается без скрытия окна. Ветки ghost/show, таймаута и ошибки
  выполняются только для UI-захвата (`restore = true`).
- Полноэкранный снимок из трея не скрывает окно; ошибки и таймаут не показывают его.
- Запись области из трей-оверлея передаёт `hide: Some(false)` и `recording_quiet = true`.

## Файлы

- `crates/app/src/app/command.rs`, `recording.rs` — `hide` и `recording_quiet`.
- `crates/app/src/app/capture.rs` — отдельные режимы запуска с `restore`/`hide`, гейты ошибок.
- `crates/app/src/app/actions.rs` — трейные запуски и остановки без изменения окна.
- `crates/app/src/app/ui_tests.rs` — тесты трея и фоновых захватов.

## Проверка

- `cargo test -p mediaa --lib`: 257.
- `cargo test -p mediaa-core --lib`: 137.
- `cargo clippy --workspace --all-targets`, `cargo fmt --all`.
