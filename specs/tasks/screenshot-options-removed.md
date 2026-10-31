# «После снимка»: убрать опции и починить скрытие окна

## Решение

- Карточка «После снимка» на странице «Скриншот» удалена: действие, формат и «Скрывать это окно»
  больше не настраиваются.
- Поведение зафиксировано: PNG, «копировать и сохранить», окно всегда скрывается перед снимком.
  Поля `ScreenshotSettings` удалены из `Settings`; старый блок `[screenshot]` в toml игнорируется.
- Снимок области раньше планировался с `hide: false`, поэтому главное окно попадало в кадр.
  Теперь область, полный экран и окно всегда идут через `schedule(..., hide = true)`: окно
  скрывается сразу, затем `HIDE_DELAY` (300 мс), и только потом снимается кадр и показывается
  оверлей.

## Файлы

- `crates/core/src/settings.rs` — удалены `ScreenshotSettings`, поле `screenshot`, `AfterCapture::ALL`.
- `crates/app/src/pages/capture.rs` — удалена карточка, мониторы занимают всю ширину.
- `crates/app/src/app/capture.rs` — фиксированные PNG/CopyAndSave, скрытие с задержкой для всех
  трёх видов снимка.
- `crates/app/src/app/window.rs` — используется существующий `schedule` и `HIDE_DELAY`.
- `crates/app/src/i18n.rs`, локали — удалены ключи `after.*`, `shot.after/action/format/hide_window`.
- Снапшоты `capture-page-wide`, `capture-page-minimum` обновлены после просмотра diff.

## Проверка

- `cargo test -p mediaa-core --lib`: 137.
- `cargo test -p mediaa --lib`: 250.
- `cargo clippy --workspace --all-targets`, `cargo fmt --all`.
