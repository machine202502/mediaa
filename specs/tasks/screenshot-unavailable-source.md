# Скриншот недоступного дисплея или окна

## Цель

На странице «Захват → Скриншот» снимок дисплея или окна, которые в момент нажатия недоступны
(дисплей отключён, окно свёрнуто, скрыто или закрыто), не делается. Вместо снимка появляется
уведомление с уровнем `Warning`.

## Решение

- `shell/capture.rs::start_fullscreen_screenshot_with_completion` перед `schedule` проверяет
  `screen::monitor_state(monitor.id)`; `MonitorState::Disconnected` → `warning` и выход. Окно не
  скрывается, capture не запускается.
- `shell/capture.rs::start_window_screenshot` проверяет
  `screen::window_status(window.id, window.pid, &window.app)`; любое состояние кроме `Available`
  (`Minimized`, `Hidden`, `Missing`, `Mismatch`) → `warning` и выход.
- `CaptureIntent::Window` теперь несёт `WindowInfo`, а не только `CaptureWindowId`: для проверки
  идентичности нужны `pid` и `app`, а для текста — заголовок.
- Текст уведомления: `toast.shot_source_unavailable` в `en.toml` и `ru.toml`.

Проверка выполняется в момент нажатия, до скрытия окна и до постановки capture в очередь. Поэтому
недоступный источник не прячет окно и не создаёт пустой файл.

## Тест-хуки

`App` в тестовой сборке получает `shot_monitor_state`/`shot_window_state` — заменяемые функции
состояния источника. В `App::new_for_test` они по умолчанию возвращают `Available`, чтобы
существующие UI-тесты снимков не зависели от реального рабочего стола; новые тесты подменяют
нужное состояние.

## Затронутые файлы

- `crates/app/src/shell/capture.rs` — preflight и хелперы доступности;
- `crates/app/src/shell.rs` — тестовые хуки состояния источника;
- `crates/app/src/shell/test_support.rs` — значения хуков по умолчанию в тестах;
- `crates/app/src/logic/intent.rs`, `crates/app/src/ui/pages/capture.rs`,
  `crates/app/src/shell/actions.rs` — `CaptureIntent::Window(WindowInfo)`;
- `crates/app/locales/{en,ru}.toml` — `toast.shot_source_unavailable`;
- `crates/app/src/shell/ui_tests.rs` — два UI-теста.

## Проверка

- `cargo test -p mediaa --lib -- warns_and_takes` — отключённый дисплей и свёрнутое окно: нет
  `pending`, capture не запущен, есть предупреждение;
- `cargo test -p mediaa --lib` — 386 passed;
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`.

## Материальные риски

- Проверка выполняется при нажатии, а сам кадр снимается после `HIDE_DELAY`. Если источник пропал
  в эти 300 мс, сработает обычная ошибка захвата (`err.capture_failed_detail`), а не warning.
- То же предупреждение теперь возможно для снимка из трея и по хоткею: они идут через
  `start_fullscreen_screenshot*` и раньше в такой ситуации получали ошибку захвата.
