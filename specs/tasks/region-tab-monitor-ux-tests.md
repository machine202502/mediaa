# UX-тесты переключения мониторов через Tab

## Статус

Завершено 26.10.26 06:33.

## Цель

Проверить через настоящий egui UX harness два пользовательских контракта:

- при одном мониторе повторный `Tab` ничего не меняет и не блокирует region action;
- при нескольких мониторах `Tab` стабильно переключает source и сохраняет активный editor.

## Контекст

- `Tab` является глобальным shortcut внутри overlay.
- Для ещё не захваченного монитора App выполняет `CaptureKind::OverlayMonitor`.
- Fallback `switch_monitor` ошибочно выбирал текущий единственный монитор и запускал его capture.
- UX harness позволяет отправлять реальные keyboard/pointer events и наблюдать итоговые state/action.

## Проверка

- Повторные `Tab` на одном мониторе не создают `CaptureMonitor`; затем selection и Copy создают
  `Outcome::Image`.
- На трёх мониторах последовательность `Tab` проходит 0 → 1 → 2 → 0 без потери `shown`.
- Overlay tests, app UI tests, Clippy и format.

## Результат

- `Tab` при одном мониторе является no-op и не запускает повторный capture;
- после repeated `Tab` selection и Copy продолжают работать;
- при нескольких мониторах порядок источников строго соответствует списку monitors;
- отсутствующий следующий кадр захватывается один раз, уже захваченный выбирается сразу;
- переключение не скрывает overlay и не оставляет switching state.

## Выполненные проверки

- `cargo test --workspace` — 124 passed в app crate;
- `cargo test -p mediaa --lib app::ui_tests` — 35 passed;
- `cargo test -p mediaa --lib overlay::tests::` — 8 passed;
- `cargo clippy --workspace --all-targets -- -D warnings` — успешно;
- `cargo fmt --all -- --check` — успешно.
