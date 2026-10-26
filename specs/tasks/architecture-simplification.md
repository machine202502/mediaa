# Переход к владельцам функций

## Статус

Основная миграция завершена 26.10.2026 коммитом `07d4178`.

## Контекст

До миграции `App` и глобальный `TaskSupervisor` одновременно владели состоянием и фоновыми
операциями. Conversion, recording, capture и library были устроены по-разному, а terminal result
мог обрабатываться в реестре и у локального потребителя. Перенос файлов без изменения владения
не считался улучшением.

## Решение

- Каждая пользовательская функция имеет одного владельца состояния и работы.
- `RecordingController`, `CaptureController`, `ConversionController` и `LibraryController`
  находятся в `crates/app/src/features/`.
- Одноразовые операции используют локальный `Job<T>` и poll только после `is_finished`.
- Recording `Saving` удерживает finalizer handle до terminal event и cleanup.
- Converter estimate использует локальный job с generation/debounce; скрытая страница не
  останавливает пользовательскую конвертацию.
- Library load/scan, removal, viewer image/audio и image copy принадлежат library/capture.
- Page UI возвращает локальные действия; `App` выполняет межфункциональную навигацию и внешние
  эффекты.
- Глобальные `TaskSupervisor`, `TaskId`, `TaskKind`, completed history и `ActiveTasks` удалены.
- Clipboard и coalescing settings writer перенесены в `services/`.

## Настройки и WebP

- FPS и output height принадлежат `RecordSettings`.
- Persisted animation quality содержит только GIF colors/dither и WebP quality.
- Runtime encoder settings собираются один раз.
- Preferences WebP отделены от runtime mode и конкретной попытки Direct/Segmented.
- Permanent normalize/validate и panic прекращают сегментную попытку без повторного split.
- Budget/resource failure остаётся retryable; неизвестная FFmpeg-ошибка сохраняет прежний
  ограниченный retry.

## Решения по гипотезам

- H1: thumbnail atlas сохраняется.
- H2: periodic rescan сохраняется; отдельно нужно оптимизировать synchronous `apply_scan`.
- H3: live WebP сохраняется.
- H4: budget guard сохраняется; CAS reservation пока не удаляется.
- H5: typed permanent failure реализован.
- H6: clipboard timeout/restart и `SettingsWriter` сохраняются без изменений.
- H7: SQLite сохраняется.

## Проверка

- `cargo test --workspace`;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo fmt --all -- --check`;
- app UI interaction, responsive, light/Russian/DPI и snapshot tests;
- реальные media pipeline tests для recording, conversion, WebP и fallback.

## Остаточный риск

Периодический rescan остаётся межплатформенной страховкой от silent watcher event loss.
MemoryBudget не является измерением RSS. FFmpeg Internal/OOM не классифицируется по тексту без
typed signal. Live platform tests требуют реального desktop session.
