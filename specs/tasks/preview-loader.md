# Индикатор генерации превью

## Поведение

- В карточке библиотеки, пока hover-прокси генерируется, поверх миниатюры крутятся три
  маленькие точки вокруг центральной. Текста и пояснений нет.
- В просмотрщике при открытии, пока нет первого кадра, показывается тот же индикатор.
- Если видео уже играет, но полный preview-прокси ещё строится в фоне, индикатор рисуется
  маленьким в правом верхнем углу.
- После готовности прокси индикатор исчезает; для карточки следующее наведение уже играет
  готовый hover-прокси.

Индикатор анимируется через `egui` painter и запрашивает repaint каждые 16 мс.

## Файлы

- `crates/app/src/theme.rs` — `preview_loader`.
- `crates/app/src/features/library/grid.rs` — индикатор в карточке.
- `crates/app/src/features/library/viewer.rs` — центральный и угловой индикаторы.
- `crates/app/src/features/library/hover_preview.rs` — `is_generating`.
- `crates/app/src/features/library/player.rs` — `preview_pending`/`preview_generating`.

## Тесты

- `hover_preview::tests::missing_proxy_keeps_full_duration_without_a_spinner` проверяет, что
  до готовности прокси показывается не спиннер, а состояние генерации.
- `player::tests::preview_generation_indicator_follows_the_background_flag`.
- app: **263**, core **138**, media pipeline **30**; `clippy` чистый.
