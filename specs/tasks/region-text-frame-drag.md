# Рамка и перетаскивание активного текста

## Статус

Завершено 26.10.26 06:20.

## Цель

Пока пользователь вводит annotation text, показывать вокруг редактора явную рамку. Рамку можно
перетаскивать мышью вместе с текстом. Клик вне рамки завершает редактирование и фиксирует текст,
но не создаёт новую text annotation.

## Контекст

- `TextEdit` хранит незафиксированный text `Shape` отдельно от `State.shapes`.
- Текущий редактор не имеет видимой рамки и не различает клик снаружи и завершение ввода.
- Text coordinates уже задаются в pixels полного кадра и не зависят от selection.

## Решение

- Отрисовывать активный text editor в рамке цвета UI accent.
- Считать полосы внутреннего margin рамки drag handle и перемещать text origin на pointer delta.
- Клик вне рамки вызывает обычный commit; background click при наличии активного текста только
  завершает его.
- Потеря фокуса вне рамки также завершает editing; перетаскивание рамки не завершает его.

## Проверка

- UI test вводит текст, перетаскивает его за рамку и проверяет итоговые coordinates.
- UI test кликает вне рамки и проверяет, что text committed и новый editor не создан.
- Existing region annotation/save tests, Clippy и format.

## Результат

- активный text editor отображается в accent frame;
- drag по полям рамки перемещает text в pixels полного кадра;
- click вне рамки фиксирует text и не создаёт следующий editor;
- drag рамки не завершает editing;
- active editor не использует тёмную подложку;
- text origin в active editor совпадает с coordinate committed shape;
- `Shift+Enter` добавляет новую строку, не завершая editing;
- existing region selection/annotation/save scenario продолжает проходить.

## Выполненные проверки

- `cargo test --workspace` — успешно, 121 app test; live desktop tests пропущены;
- `cargo test -p mediaa --lib app::ui_tests` — 35 passed;
- `cargo test -p mediaa --lib overlay::tests::` — 5 passed;
- `cargo clippy --workspace --all-targets -- -D warnings` — успешно;
- `cargo fmt --all -- --check` — успешно;
- локальные Markdown links и `git diff --check` — успешно.
