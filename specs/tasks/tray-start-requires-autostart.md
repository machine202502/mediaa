# «При запуске открывать в трее» только при автозапуске

## Решение

- Галочка «При запуске открывать в трее» неактивна, пока не включено «Запускать при старте
  системы»: `start_in_tray` имеет смысл только для автозапуска.
- В `Settings::sanitized` добавлен инвариант: при `launch_at_login = false` поле
  `start_in_tray` сбрасывается в `false`. Это применяется при загрузке старых настроек.
- При выключении автозапуска интерфейс сразу сбрасывает `start_in_tray`, чтобы не осталось
  скрытого включённого значения.

## Файлы

- `crates/core/src/settings.rs` — инвариант в `sanitized`, тест на загрузку.
- `crates/app/src/pages/settings.rs` — `add_enabled` для чекбокса трея.
- `crates/app/src/app/ui_tests.rs` — проверка `disabled` и обновлённое ожидание.
- Снапшоты `settings-*` обновлены после просмотра diff (неактивная галочка).

## Проверка

- `cargo test -p mediaa-core --lib`: 136.
- `cargo test -p mediaa --lib`: 250.
- `cargo clippy --workspace --all-targets`, `cargo fmt --all`.
