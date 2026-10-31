# Строки окон в захвате

## Решение

В разделе `Capture → Screenshot → Windows` строка окна должна быть такой же кликабельной
кнопкой, как строка дисплея: иконка камеры и название на всю ширину.

Раньше кликабельной была только иконка камеры, а название — отдельной меткой.

## Изменения

- `crates/app/src/pages/capture.rs` — окно рендерится как `theme::secondary` на всю ширину;
- `crates/app/src/app/ui_tests.rs` — клик выполняется по строке окна;
- `crates/app/tests/snapshots/capture-page-*.png` — обновлены.

## Проверка

- `cargo test -p mediaa --lib` — 268;
- UI-снапшоты capture-page-wide/minimum;
- `clippy` чистый.
