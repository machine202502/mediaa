# Запись из трея не трогает окно

## Решение

- `RecordingCommand::Start` получил `hide: Option<bool>`.
- Запуски из трея (`StartRecording`, `StartAudio`) передают `Some(false)`: окно не скрывается и
  не показывается, даже если было открыто.
- UI-запись и хоткеи передают `None` — работает прежняя логика профиля (`hide_to_tray`).
- Запуск записи из оверлея области использует `Some(false)`: окно уже обработано оверлеем.
- Трей-снимки области/экрана не менялись.

## Файлы

- `crates/app/src/app/command.rs`, `recording.rs` — параметр `hide`.
- `crates/app/src/app/actions.rs` — `Some(false)` для трея, `None` для UI/хоткеев.
- `crates/app/src/pages/mod.rs` — UI-аудио без override.
- `crates/app/src/app/ui_tests.rs` — тест, что запись из трея не трогает окно.

## Проверка

- `cargo test -p mediaa --lib`: 257.
- `cargo test -p mediaa-core --lib`: 137.
- `cargo clippy --workspace --all-targets`, `cargo fmt --all`.
