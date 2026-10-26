# Плавное открытие region overlay

## Статус

В работе с 26.10.26 06:39.

## Цель

Убрать моргание и визуальное масштабирование при первом показе region editor без перехода к
постоянно прогретому native окну.

## Контекст

- Overlay является отдельным native viewport и создаётся после захвата монитора.
- Размер и позиция монитора уже передаются в `ViewportBuilder` до первого показа.
- Текущий GPU surface не поддерживает transparent composite mode, что даёт предупреждение.
- Скрытый viewport создаётся активным, хотя показывается только через два кадра.

## Первый вариант

- Явно отключить transparency у overlay viewport.
- Сделать viewport non-resizable.
- Не активировать скрытый viewport; активировать только после reveal.
- Отключить DWM transitions на первом кадре скрытого окна.

Первый вариант не устранил артефакт при визуальной проверке.

## Второй вариант

- Создавать overlay сразу как borderless fullscreen viewport.
- Передавать целевой monitor index до первого показа.
- Сохранить opaque/non-resizable/non-active-until-shown поведение.

Второй вариант вызвал panic egui-wgpu в staging buffer и был откатан.

## Третий вариант

- Создавать стабильный скрытый overlay viewport заранее в idle UI lifecycle.
- Не менять `active` между hidden и shown builder, чтобы eframe не пересоздавал native window.
- При вызове региона переиспользовать прогретый viewport и показывать его без DWM animation.

## Результат

- prewarmed overlay появляется после вызова региона;
- пользователь визуально подтвердил вариант как рабочий;
- fullscreen-эксперимент удалён;
- после замены prewarm callback на реальный Overlay root и viewport явно запрашивают repaint.

## Выполненные проверки

- `cargo test --workspace` — 124 passed в app crate;
- `cargo test -p mediaa --lib overlay::tests::` — 8 passed;
- `cargo clippy --workspace --all-targets -- -D warnings` — успешно;
- `cargo fmt --all -- --check` — успешно;
- ручная визуальная проверка пользователем — успешно.

## Проверка

- Unit test проверяет opaque/non-resizable/activity contract builder.
- Existing overlay tests, app UI tests, Clippy, format и ручная визуальная проверка пользователем.
