# UI/UX regression tests

## Статус

Основной контур реализован; поддерживается по мере изменения UI.

## Контекст

Реальный захват, трей, глобальные hotkeys и OS permissions нельзя использовать в обычных UI
tests. До изменения runtime lifecycle потребовался контур, который запускает настоящие egui
экраны и контроллеры с детерминированными устройствами.

## Решение

- Использовать `egui_kittest::Harness`, а не отдельную копию UI на mock-widgets.
- Создавать `App` через test-only фабрику с `TestCatalog`.
- Подставлять временную библиотеку, фиксированные displays/windows/microphones и отключённые
  native-сервисы.
- Искать и нажимать элементы через AccessKit/kittest.
- Выполнять общие interaction/geometry tests на всех платформах.
- Pixel snapshots выполнять на Windows с фиксированными OS, размером и темой.
- Не снимать hover и движущиеся индикаторы.
- Обновлять baselines только после визуальной проверки.

## Команды

```powershell
cargo test -p mediaa --lib app::ui_tests
UPDATE_SNAPSHOTS=true cargo test -p mediaa --lib ui_regression_snapshot
scripts/windows/region-overlay-smoke.ps1
```

## Покрытие

- запись screen/window/audio, pause/resume/save/cancel;
- screenshot fullscreen/window/region editor;
- dynamic region text/menu/palette/edge selection и native overlay smoke;
- settings, profiles, microphones, tray policy;
- library filtering, search, sorting, actions и deletion;
- conversion start/progress/cancel/terminal state;
- WebP fallback notice и trace;
- single-instance dev isolation;
- responsive layouts, DPI, light theme и Russian UI.
- populated Library/Convert и минимальные Screenshot/Library/Convert/Settings.

## Ограничения

Этот контур не заменяет реальные platform capture tests и не проверяет системные permission
dialogs. Реальные `RecordingSession` внутри одного test binary сериализуются: параллельные
FFmpeg-сессии нестабильны. Locale глобален, поэтому App harnesses сериализуются общим mutex.
