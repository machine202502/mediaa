# Приоритетные пробелы UX-тестов

## Статус

В работе с 26.10.26 07:00.

## Цель

Закрыть только те пробелы, где реальные UX-баги уже проходили мимо зелёного suite:

- native lifecycle region overlay;
- dynamic states text/menu/edge selection;
- populated Library и выбранный Convert;
- вертикальные наложения responsive layout.

## Контекст

- Overlay unit tests напрямую вызывают `State::ui` и не проверяют native viewport.
- Единственный визуальный baseline region editor содержит обычную selection с готовой стрелкой.
- Library/Convert snapshots показывают только empty states.
- Responsive tests проверяют title и горизонтальные границы, но не перекрытие fixed chrome.

## Решение

- Добавить визуальные baselines для active multiline text, text context menu, color palette и
  selection у края.
- Добавить snapshots populated Library и выбранного conversion target.
- Добавить geometry test отсутствия вертикальных пересечений fixed chrome на минимальном окне.
- Добавить отдельную native smoke-проверку prewarmed overlay с уникальным dev instance.

## Проверка

- Новые snapshots сначала обновить, затем просмотреть глазами и только после этого принять.
- UI/overlay tests, Clippy, format и native smoke.

## Результат

- добавлены snapshots active multiline text, text menu, palette и edge selection;
- добавлены snapshots populated Library и выбранного Convert;
- добавлены минимальные snapshots Screenshot, Library, Convert и Settings;
- добавлен interaction-test дополнительных Settings toggles;
- добавлен native smoke script для prewarmed overlay и `Tab`.

Визуальный просмотр нашёл отдельный UX-дефект: selection size badge может перекрывать active text
frame, если перетащить текст через верхнюю границу selection. Snapshot фиксирует это состояние;
исправление продукта в test-only задачу не входило.

## Выполненные проверки

- `cargo test --workspace` — 134 passed в app crate;
- `cargo test -p mediaa --lib app::ui_tests` — 42 passed;
- `cargo test -p mediaa --lib overlay::tests::` — 11 passed;
- `scripts/windows/region-overlay-smoke.ps1` — passed;
- `cargo clippy --workspace --all-targets -- -D warnings` — успешно;
- `cargo fmt --all -- --check` — успешно.
