# Отдельные крейты форматов

## Решение

Работа с WebP и GIF живёт в отдельных крейтах, а `mediaa-media` оркестрирует запись,
конвертацию, аудио и preview.

Слои:

- `mediaa-ffmpeg` — запуск FFmpeg и отмена (`job`), общий build script для линковки.
- `mediaa-webp` — WebP-граф, normalize/validate/trim/merge animated WebP.
- `mediaa-gif` — GIF-граф, палитра и dither.
- `mediaa-media` — recorder, convert, audio, preview; зависит от трёх крейтов выше.

`webp-pipeline` удалён: сегментный scheduler больше не нужен, а `webp_parts` переехал
в `mediaa-webp`.

## Что удалено

- крейт `webp-pipeline` и его тесты/примеры;
- зависимость `mediaa-webp-pipeline`;
- `gif_zengif` и зависимость `zengif` (замеры показали, что он медленнее FFmpeg).

## Проверки

- `cargo test -p mediaa-ffmpeg -p mediaa-webp -p mediaa-gif`;
- `cargo test -p mediaa-core -p mediaa-media -p mediaa`;
- `cargo fmt --check`.
