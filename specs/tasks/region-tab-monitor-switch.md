# Переключение мониторов через Tab

## Статус

Завершено 26.10.26 06:30.

## Симптом

Нажатие `Tab` в region editor мигает приложением и заново показывает регион. Несколько повторных
нажатий могут оставить приложение в состоянии, когда tray и UI не открывают region editor.

## Контекст

- `Tab` для ещё не захваченного монитора возвращает `Outcome::CaptureMonitor`.
- App запускает `CaptureKind::OverlayMonitor`, не убирая текущий overlay.
- Viewport ID включ��ет индекс монитора, поэтому смена кадра создаёт новый native viewport.
- Overlay остаётся `Some` при ошибке или таймауте monitor capture, и последующие region actions
  молча завершаются.

## Решение

- Использовать стабильный native overlay viewport.
- Передавать в `ViewportBuilder` текущую visibility, а не постоянно `false`.
- Не запускать второй monitor capture, пока предыдущий не завершён.
- Сбрасывать switching state после ошибки или таймаута.
- Не заново выполнять reveal уже показанного overlay при смене содержимого монитора.

## Проверка

- UI test подтверждает блокировку повторного `Tab` до готовности кадра.
- Unit test подтверждает восстановление после неудачного monitor capture.
- Existing app UI capture/save scenarios, Clippy и format.

## Результат

- overlay сохраняет один native viewport при смене монитора;
- stable viewport остаётся visible после первого показа;
- успешное переключение не выполняет повторный reveal и не скрывает уже показанный editor;
- повторный `Tab` во время capture игнорируется;
- timeout, ошибка worker и ошибка запуска снимают switching state;
- tray/UI больше не остаются заблокированными скрытым `Overlay`.

## Выполненные проверки

- `cargo test --workspace` — успешно, 122 app test; live desktop tests пропущены;
- `cargo test -p mediaa --lib overlay::tests::` — 6 passed;
- `cargo clippy --workspace --all-targets -- -D warnings` — успешно;
- `cargo fmt --all -- --check` — успешно.
