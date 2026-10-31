# UI-захват возвращает окно после оверлея

## Решение

- У region overlay появился флаг `restore`: он ставится только когда снимок области или выбор
  области записи запущены из интерфейса.
  - `start_region_screenshot` (кнопка в UI) → `restore = true`.
  - `start_region_screenshot_from_hotkey` (хоткей/CLI `--area`) → `restore = false`.
  - `start_region_screenshot_with_completion` (трей) → `restore = false`.
- После закрытия оверлея `handle_overlay` вызывает `show_window`, если `restore` включён. Это
  работает для «Копировать», «Сохранить», «Сохранить…», отмены и выбора области записи.
- Для выбора области записи окно возвращается, страница переключается на «Запись»; при запуске
  записи из оверлея окно также остаётся видимым.
- Запуски из трея и хоткеев не показывают окно, как и раньше.

## Файлы

- `crates/app/src/app/capture.rs` — три режима запуска и передача `restore`.
- `crates/app/src/app/state.rs`, `features/capture/mod.rs`, `overlay.rs` — флаг в `Pending`,
  `CaptureKind`, `Overlay`.
- `crates/app/src/app/actions.rs` — `show_window` после UI-оверлея, хоткей/CLI без restore.
- `crates/app/src/app/ui_tests.rs` — тесты restore и отсутствия restore для фона.

## Проверка

- `cargo test -p mediaa --lib`: 256.
- `cargo test -p mediaa-core --lib`: 137.
- `cargo clippy --workspace --all-targets`, `cargo fmt --all`.
