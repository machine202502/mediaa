# Копирование из region overlay всегда сохраняет в библиотеку

## Решение

- Кнопка «Копировать» и Enter в оверлее области делают одно и то же: сохраняют PNG в библиотеку
  и копируют изображение в буфер. Действие зафиксировано `AfterCapture::CopyAndSave`; выбора
  «только скопировать» больше нет.
- «Сохранить…» по-прежнему открывает системный диалог и пишет только выбранный файл.
- В списке клавиш страницы настроек Ctrl+C теперь описан как «Копировать и сохранить».

## Файлы

- `crates/app/src/overlay/editor.rs` — Copy/Ctrl+C используют `CopyAndSave`.
- `crates/app/src/overlay.rs` — тестовый harness и проверки действия; Enter проверяется отдельно.
- `crates/app/locales/{ru,en}.toml` — описание Ctrl+C.
- Снапшоты `settings-*` обновлены после просмотра diff (строка списка клавиш).

## Проверка

- `cargo test -p mediaa --lib`: 252.
- `cargo test -p mediaa-core --lib`: 136.
- `cargo clippy --workspace --all-targets`, `cargo fmt --all`.
