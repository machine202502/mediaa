# Завершение записи всегда копирует файл и кладёт его в библиотеку

## Решение

- Из настроек удалена карточка «После завершения» и выбор `AfterCompletionAction`
  (`OpenLibrary`/`CopyFile`). Поле `Settings.after_completion` удалено; старое значение в toml
  игнорируется.
- После успешного завершения записи файл всегда:
  1. добавляется в библиотеку (`lib.files_changed`) и открывается страница «Библиотека», если
     приложение не завершается после записи;
  2. копируется в буфер как файл (`Purpose::Recording` с уведомлением «Сохранено: имена»).
- Снимки с `after_completion` (хоткей/трей) используют то же поведение: файл копируется и
  библиотека обновляется.

## Файлы

- `crates/core/src/settings.rs` — удалены `AfterCompletionAction`, поле и тест.
- `crates/app/src/app/recording.rs` — завершение всегда копирует файлы и открывает библиотеку.
- `crates/app/src/app/capture.rs` — после завершения снимка файл всегда копируется.
- `crates/app/src/pages/settings.rs`, локали — карточка и ключи удалены.
- Снапшоты `settings-*` обновлены после просмотра diff.

## Проверка

- `cargo test -p mediaa-core --lib`: 136.
- `cargo test -p mediaa --lib`: 250; `stop_from_tray_uses_the_same_save_flow` проверяет переход
  в библиотеку после завершения.
- `cargo clippy --workspace --all-targets`, `cargo fmt --all`.
