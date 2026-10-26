# Аннотации вне выбранной области

## Статус

Завершено 26.10.26 05:53.

## Цель

В screenshot editor разрешить pen, marker, line, arrow, rectangle и text начинаться в любой точке
полного кадра, даже если selection уже существует. Preview должен показывать annotations за
пределами selection. Позднее selection можно переместить, уменьшить или расширить, не теряя
аннотации.

## Контекст

- `interact` фильтровал начало инструментов через `selection.contains`.
- `paint` обрезал все shapes painter clip rect текущего selection.
- shape coordinates уже хранятся в pixels полного кадра.
- `render_crop` переводит полные coordinates в координаты итогового crop.

## Решение

- selection ограничивает только итоговый crop.
- Инструменты и text editor работают во всём viewport.
- Preview paints shapes без clip по selection.
- Move/resize selection остаётся только для cursor tool.
- Итоговый файл по-прежнему содержит только selection; annotations вне итогового crop естественно
  не попадают в файл, но остаются доступны при последующем изменении selection.

## Проверка

- UI test рисует arrow и открывает text editor вне существующего selection.
- Render test показывает annotation после расширения crop.
- Existing region selection/annotation/save test продолжает проходить.
- Clippy и format tests.

## Результат

- arrow и text editor успешно открываются вне selection;
- preview больше не clip-ает shapes по выбранной области;
- render test подтверждает, что расширение crop показывает ранее внешнюю annotation;
- existing selection/annotation/save UI scenario проходит.

## Выполненные проверки

- `cargo test --workspace` — успешно; live desktop tests пропущены без desktop session;
- `cargo test -p mediaa --lib app::ui_tests` — 35 passed;
- `cargo test -p mediaa --lib overlay::tests::` — 4 passed;
- targeted render test — passed;
- `cargo clippy --workspace --all-targets -- -D warnings` — успешно;
- `cargo fmt --all -- --check` — успешно;
- локальные Markdown links и `git diff --check` — успешно.
