# Тесты многоканальной записи и per-source fallback

## Новые тесты

Фикстуры: `crates/media/tests/fixtures/fake-monitor.webp` и `fake-window.webp` — короткие
зацикленные WebP-анимации. Тестовый `VideoSource` декодирует их, масштабирует под «монитор» или
«окно» и отдаёт кадры как обычный `FrameSource`; реальный захват не нужен.

- `live_webp_handles_three_large_monitors_without_hanging` — три источника 1280×720.
- `live_webp_handles_two_large_windows_without_hanging` — два источника 960×540.
- `one_failed_live_source_does_not_force_the_other_offline` — второй live-источник специально
  сломан; первый остаётся на live-пути (`stage=live source=0 merged=webp`), второй кодируется
  offline.
- `both_failed_live_sources_fall_back_to_offline_webp` — оба live-источника сломаны, оба уходят
  в offline и дают валидные файлы.
- `closing_one_video_source_finishes_without_hanging` — источник возвращает `window closed`;
  финализация завершается по таймауту теста, без зависания.
- `finish_with_timeout` запускает финализацию в отдельном потоке и падает по таймауту 40/60 с.
- `two_source_fallbacks_use_the_linear_encoder_independently` — для двух источников
  segmented-энкодер падает, каждый независимо переходит на linear (Direct).

## Per-source fallback

`LiveResult` теперь хранит `errors: Vec<Option<String>>` по источникам. Менеджер live-пути не
останавливает все воркеры из-за одного источника: он помечает только сбойный, остальные
продолжают кодировать. Recorder для каждого источника отдельно решает:

- есть валидные live-части без ошибки — мержит live;
- иначе кодирует этот MKV offline.

Если все источники live-успешны, поведение прежнее: сразу отдаются live-файлы.

## Проверка

- `cargo test -p mediaa-media --test pipeline`: 30.
- `cargo test -p mediaa-media --lib`: 44.
- `cargo test -p mediaa-webp-pipeline`: 17.
- app: 258, core: 137.
- `cargo clippy --workspace --all-targets`, `cargo fmt --all`.
