# Закрытие окна по умолчанию

## Решение

`Settings::close_to_tray` по умолчанию `false`: закрытие окна не скрывает приложение в трей,
пока пользователь сам не включит настройку.

## Изменения

- `crates/core/src/settings.rs` — default и тест загрузки старого файла;
- `crates/app/tests/snapshots/settings-*.png` — чекбокс выключен;
- `crates/app/src/app/ui_tests.rs` — клик по чекбоксу теперь включает его.

## Проверка

- `cargo test -p mediaa-core` — 138;
- `cargo test -p mediaa --lib` — 268;
- `cargo clippy --workspace --all-targets` чистый.
