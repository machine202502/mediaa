# Модалки блокируют интерфейс

## Решение

- Оконные модалки (`contract`, task details, speaker dialogs, analysis) получили
  `Order::Foreground`, прозрачный backdrop на весь экран и `set_modal_layer`. Фон остаётся
  нарисованным как обычно, без затемнения.
- При открытой модалке `pages::shell` фильтрует ввод до отрисовки страниц: pointer-события вне
  прямоугольника модалки отбрасываются, клавиатура отбрасывается, если модалка не содержит
  текстового ввода. Так фон не получает ни мышь, ни горячие клавиши.
- `egui::Modal` (чат, каталоги, библиотека) тоже регистрирует свой прямоугольник, чтобы клавиши
  не утекали в фон.
- Уведомления при активной модалке рисуются в режиме `show_blocked`: остаются видимыми, но
  `ui.disable` не даёт нажимать кнопки, пока модалка не закрыта.

## Файлы

- `crates/app/src/theme.rs` — `mark_modal`, `mark_modal_rect`, `modal_backdrop`,
  `block_background_input`.
- `crates/app/src/pages/mod.rs` — фильтрация ввода в начале `shell`.
- `crates/app/src/pages/about.rs`, `agent/{task_details,dialog,analysis}.rs` — backdrop и modal
  layer для окон.
- `crates/app/src/app/notices.rs` — `show_blocked`.
- `crates/app/src/app.rs` — уведомления блокируются при активной модалке.
- `crates/app/src/pages/agent/{chat,catalogs}.rs`, `features/library/actions.rs` — регистрация
  прямоугольников `egui::Modal`.

## Проверка

- `the_transcription_provider_has_the_contract_info` проверяет, что клик по навигации не меняет
  страницу при открытой модалке.
- `blocked_notices_are_disabled` проверяет неактивные кнопки уведомлений.
- app: 258, core: 137.
- `cargo clippy --workspace --all-targets`, `cargo fmt --all`.
