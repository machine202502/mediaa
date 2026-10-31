# Отдельные крейты форматов

## Решение

Работа с FFmpeg, WebP и GIF живёт в отдельных крейтах, а `mediaa-media` оркестрирует
запись, конвертацию, аудио и preview.

Слои:

- `mediaa-ffmpeg` — единственный владелец `ez-ffmpeg`: job builder/start/wait, probe,
  frame export, push-источник, PCM-декодирование, capabilities, отмена и build script
  для линковки.
- `mediaa-webp` — WebP-граф, декодер, поток кадров, кодирование,
  normalize/validate/trim/merge animated WebP.
- `mediaa-gif` — GIF-граф, кодирование и trim.
- `mediaa-media` — recorder, convert, audio, preview; зависит от трёх крейтов выше и не
  импортирует `ez-ffmpeg` или `image-webp`.

`webp-pipeline` удалён: сегментный scheduler больше не нужен, а `webp_parts` переехал
в `mediaa-webp`.

## Что удалено

- крейт `webp-pipeline` и его тесты/примеры;
- зависимость `mediaa-webp-pipeline`;
- `gif_zengif` и зависимость `zengif` (замеры показали, что он медленнее FFmpeg);
- дублирующий `build.rs` и прямая зависимость `mediaa-media` от `ez-ffmpeg`/`image-webp`.

## Проверки

- `cargo test -p mediaa-ffmpeg -p mediaa-webp -p mediaa-gif`;
- `cargo test -p mediaa-core -p mediaa-media -p mediaa`;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo fmt --check`.
