# «Скрывать в трей во время записи» только на странице записи

## Решение

- В общем редакторе профиля записи (`pages/record.rs::profile_editor`) галочка
  «Скрывать в трей во время записи» показывается только для `RecordProfile::App`. На странице
  настроек профили `Hotkey` и `Region` её больше не рисуют: запуск из трея и хоткея и так не
  показывает окно.
- Для записи из окна программы поведение прежнее и проверено: `request_recording` вызывает
  `schedule(..., hide = true)`, окно скрывается, выдерживается `HIDE_DELAY` (300 мс), затем
  начинается запись.
- После успешного завершения записи окно возвращается вместе со страницей «Библиотека»
  (если приложение не завершается после записи).

## Файлы

- `crates/app/src/pages/record.rs` — галочка только для `RecordProfile::App`.
- `crates/app/src/app/recording.rs` — `show_window` после успешного завершения.
- `crates/app/src/app/ui_tests.rs` — тест `starting_program_recording_hides_the_window_first`.

## Проверка

- `cargo test -p mediaa --lib`: 251.
- `cargo test -p mediaa-core --lib`: 136.
- `cargo clippy --workspace --all-targets`, `cargo fmt --all`.
