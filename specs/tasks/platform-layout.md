# Платформенный слой: Windows-код в `windows/`, будущие ОС через возможности

## Цель

Собрать весь проектный Windows-код в `crates/platform/src/windows/`, сохранить существующий
non-Windows-код рабочим и сделать публичный API таким, чтобы будущие Linux/macOS можно было
добавить без переделки фасада.

## Актуальность файлов

Перед началом каждой подзадачи и перед каждым изменением файла в `crates/core` и
`crates/app` перечитать этот файл заново. Не полагаться на содержимое, прочитанное ранее:
файлы могли быть изменены другим человеком или другой сессией. То же правило применять к
файлам `crates/platform`, которые будут затронуты.

На 01.11.26 04:36 в `crates/app` и `crates/core` есть незакоммиченные изменения. Это
актуальное состояние, а не исторический план.

## Проверка app/core перед стартом

- `cargo check -p mediaa --lib --profile dev-check` проходит (2m08s).
- `WindowResult`/`WindowOpen` уже есть в `mediaa-platform`; `app` уже разбирает
  `WindowOpen::Unsupported` и использует `set_visible(...).is_ok()`,
  `set_ghost(...).is_ok()`. Поэтому в `PLAT-2.2` не нужно заново вводить эти типы.
- `platform` уже скрывает `wgc`/`x11`/`stream`, `Cpal*`, `DeferredSource`,
  `MonitorSource`, `WindowSource`, `RegionOutlineBackend`; это результат `PLAT-1`, не
  переделывать.
- В `platform` всё ещё остались: `window::RegionOutline` как публичный дубль,
  `bool` у `exclude_from_capture`, `make_black_transparent`, `disable_open_animation`,
  `RegionOutline::show`; их и трогает `PLAT-2.2`.
- Уведомления в `app` уже расширены (`Notices::with_enabled`, уровни, важные уведомления);
  `App::error`/`warning` работают. Ошибки `screen::monitors`/`windows`/`capture_*` уже
  показываются через `err.list_displays`/`err.list_windows`/`err.capture_failed_detail`.
  В `PLAT-2.5` не нужен новый механизм уведомлений: достаточно сообщения об ошибке и
  проверки, что оно доходит до UI.
- `app/src/app/actions.rs` добавил простое выбор одной цели через `cursor::position` и
  `screen::monitor_at`; сохранять `Option`-семантику этих функций.
- `app/src/pages/settings.rs` сильно переработан, но вызовы
  `mediaa_platform::startup::configure` и `mediaa_platform::keys::print_screen_down`
  остались; `PLAT-2.3` не требует правок в settings, кроме проверки.
- `core/src/dev.rs` больше не содержит `notices_mode`; `core/src/paths.rs` и
  `core/src/settings.rs` изменены, но к платформенному слою это не относится.
- `app/src/app/ui_tests.rs` вырос; `cfg(windows)` в нём остаётся тестовым и допустимым.
- `app/Cargo.toml` target-секция `tray-icon` и `main.rs` `windows_subsystem` не менялись.

Поддержка Linux/macOS сейчас не добавляется и не проверяется. Но абстракция должна уметь
отвечать на три разных вопроса:

- операция работает;
- операция недоступна, но приложение умеет её обойти;
- операция недоступна и без неё нельзя продолжать.

## Решения

- Поддерживается только Windows. Linux/macOS не собираются и не тестируются.
- Существующий non-Windows-код не удаляется и остаётся рабочим там, где он написан:
  `x11.rs`, `stream.rs`, unix-`single_instance`, non-Windows `cursor`, `cursor_overlay::arrow`,
  `device_query`, `xcb`, `libc`.
- `clipboard`, `files`, `audio`, `startup` остаются на верхнем уровне: они построены на
  внешних межплатформенных пакетах.
- `WindowOpen`, `WindowResult`, `RegionOutline`, `cursor::current` не удаляются.
- `bool` у window effects и outline заменяется на `WindowResult`.
- `window::RegionOutline` остаётся, но становится `pub(crate)`; публичный тип —
  `outline::RegionOutline`.
- Паника — только для обязательной операции, у которой нет реализации и нет безопасного
  fallback. Сейчас это `single_instance` на неизвестной ОС.
- Если можно вернуть `Unsupported`/`Err`/`None`/`false` и приложение покажет уведомление или
  использует fallback — возвращаем это, а не паникуем.
- `platform::init()` добавляется: Windows — DPI awareness, остальные — no-op.
- `windows_subsystem`, `winresource`, `ffmpeg/build.rs` и target-секция `tray-icon`
  остаются документированными исключениями сборки.
- Вся работа — одна задача `PLAT-2`.

## Политика unsupported

| Операция | Windows | Linux/macOS сейчас | Другие ОС |
|---|---|---|---|
| Список дисплеев/окон | WGC/xcap | существующие x11/stream | `Err` → уведомление |
| Захват экрана/окна | WGC/xcap | существующие x11/stream | `Err` → уведомление |
| Источники записи | WGC/xcap | существующие x11/stream | `Err` → уведомление при старте |
| Single instance | Win32 mutex/event | unix flock/socket | `panic!` |
| Курсор | GDI | device_query | `None` |
| PrintScreen | Win32 | `false` | `false` |
| System audio | WASAPI | cpal, `false` | `false` |
| Native window | Win32 | `WindowOpen::Unsupported` | `Unsupported` |
| Window effects | Win32 | `WindowResult::Unsupported` | `Unsupported` |
| Native outline | Win32 | `WindowResult::Unsupported` | `Unsupported` |
| Autostart | auto-launch | auto-launch | ошибка в настройках |
| Clipboard/files | внешние пакеты | внешние пакеты | ошибка операции |

`screen` на неизвестной ОС не паникует: `monitors`/`windows`/`capture_*`/`*_source`
возвращают `Err("screen capture is not supported on this platform")`. Приложение уже
показывает такие ошибки через `err.list_displays`, `err.capture_failed_detail`,
`toast.recording_start_failed`.

## Публичный API

- `WindowOpen { Supported(NativeWindow), Unsupported(&'static str) }` — остаётся. На
  не-Win32 возвращает `Unsupported`; приложение логирует и работает без native window.
- `WindowResult { Ok, Unsupported, Failed }` — остаётся и используется вместо `bool`:
  - `Ok` — выполнено;
  - `Unsupported` — ОС/возможность недоступна, приложение делает fallback;
  - `Failed` — операция поддерживается, но Win32 вернул ошибку.
- `exclude_from_capture`, `make_black_transparent`, `disable_open_animation`,
  `RegionOutline::show` возвращают `WindowResult`.
- `WindowResult::Failed` должен реально возвращаться, когда Win32-вызов не удался.
- `window::RegionOutline` — `pub(crate)`; снаружи только `outline::RegionOutline`.
- `cursor::current`/`CursorState` остаются; `CursorImage` остаётся в `cursor_overlay`.

## Структура

```text
crates/platform/src/
  lib.rs
  screen/
    mod.rs               # доменный фасад
    types.rs             # MonitorInfo, WindowInfo, id, состояния
    capture.rs           # общий xcap-захват, MonitorSource/WindowSource, DeferredSource
  audio.rs
  clipboard.rs
  files.rs
  window.rs              # публичные типы + вызовы Windows-реализации
  outline.rs             # публичный RegionOutline + приватный trait
  cursor.rs
  keys.rs
  single_instance.rs     # фасад + unix-реализация
  startup.rs
  cursor_overlay.rs
  stream.rs              # существующий non-Windows захват
  x11.rs                 # существующий X11
  windows/
    mod.rs
    init.rs              # DPI awareness
    screen.rs            # WGC, Win32-состояние окна
    wgc.rs
    window.rs            # NativeWindow, ghost, capture exclusion, прозрачность, анимация
    outline.rs           # WindowsBackend
    outline_window.rs    # native layered-окно контура
    cursor.rs            # GDI-курсор
    keys.rs
    single_instance.rs
    startup.rs           # Windows-цитирование пути
    audio.rs             # system_audio_supported
```

Общий `xcap`-захват остаётся в `screen/capture.rs`: им пользуются и Windows, и существующие
x11/stream. В `windows/` уходит только Windows-специфичное.

## Подзадачи

Все подзадачи в статусе `ожидание`. Работа начинается после отдельного разрешения
пользователя, по одной подзадаче.

### PLAT-2.1 — Каркас `windows/` и screen/capture

Зависимостей нет.

- Создать `crates/platform/src/windows/`.
- Перенести `wgc.rs` → `windows/wgc.rs`.
- Разделить `screen.rs` на `screen/{mod,types,capture}.rs`.
- Общий `xcap`-захват, `MonitorSource`, `WindowSource`, `DeferredSource` оставить в
  `screen/capture.rs`.
- Windows-часть экрана (WGC, Win32-состояние окна, `capture_window` через WGC) перенести в
  `windows/screen.rs`.
- Существующие x11/stream-ветки не удалять.
- Публичный API не менять.

Проверка: `cargo check -p mediaa-platform --lib --profile dev-check`,
`cargo test -p mediaa-platform --lib --test boundary`, снимок экрана/окна на Windows.

### PLAT-2.2 — Window/outline и `WindowResult`

Зависит от `PLAT-2.1`.

- Перенести Win32-части `window.rs` → `windows/window.rs`.
- Перенести Windows-backend `outline` → `windows/outline.rs` и
  `windows/outline_window.rs`.
- `window::RegionOutline` сделать `pub(crate)`.
- Заменить `bool` на `WindowResult` у `exclude_from_capture`,
  `make_black_transparent`, `disable_open_animation`, `RegionOutline::show`.
- `WindowResult::Failed` возвращать при реальном отказе Win32.
- Не вводить заново `WindowResult`/`WindowOpen`: они уже есть, `app` уже использует
  `is_ok()` и разбирает `WindowOpen::Unsupported`.
- Обновить только `app/src/overlay.rs` и `app/src/region_marker.rs`; `app/window.rs`,
  `app/actions.rs`, `app/capture.rs` уже совместимы.

Проверка: `cargo check --workspace --lib --bins --profile dev-check`,
`cargo test -p mediaa --lib`, UI-тесты overlay/region.

### PLAT-2.3 — Cursor, keys, single_instance, startup, audio

Зависит от `PLAT-2.1`.

- GDI-курсор → `windows/cursor.rs`; non-Windows `device_query` оставить.
- Win32 `keys` → `windows/keys.rs`; non-Windows `false` оставить.
- Windows `single_instance` → `windows/single_instance.rs`; unix-реализацию оставить.
- `single_instance` на неизвестной ОС → `panic!`.
- Windows-цитирование `startup` → `windows/startup.rs`.
- `system_audio_supported` → `windows/audio.rs`; non-Windows `false`.

Проверка: `cargo test -p mediaa-platform --lib`, `cargo check --workspace --lib
--bins --profile dev-check`.

### PLAT-2.4 — `platform::init()`

Зависит от `PLAT-2.1`.

- Добавить `windows/init.rs`: DPI awareness.
- Добавить `pub fn init()`; на non-Windows — no-op.
- Перевести примеры `snap`, `capture_bench`, `webp_matrix` на `platform::init()`.
- Перенести `Win32_UI_HiDpi` из dev-dependencies в обычные Windows-зависимости.

Проверка: `cargo check -p mediaa-platform --all-targets --profile dev-check`,
запуск примеров на Windows.

### PLAT-2.5 — Unsupported-политика и уведомления

Зависит от `PLAT-2.1`–`PLAT-2.3`.

- `screen` на неизвестной ОС: `monitors`/`windows`/`capture_*`/`*_source` → `Err`
  «screen capture is not supported on this platform».
- `monitor_at` → `None`, `monitor_state` → `Disconnected`, `window_status` → `Missing`.
- Уведомления уже есть: `App::error`/`warning` и `Notices` расширены; новые ключи
  `err.list_displays`, `err.list_windows`, `err.capture_failed_detail`,
  `toast.recording_start_failed` уже используются. Новый механизм уведомлений не нужен.
- Проверить, что `screen`-ошибка доходит до `refresh_monitors`/`refresh_catalogs`/capture
  job/старта записи и показывается пользователю.
- Проверить fallback для `WindowOpen::Unsupported`, `WindowResult::Unsupported`,
  `RegionOutline::show`.
- Добавить locale-ключ, только если существующего текста недостаточно.

Проверка: unit-тест на формирование ошибки; UI-тест уведомления; проверка, что
non-Windows-ветки не паникуют там, где есть fallback.

### PLAT-2.6 — Boundary-тест и финальные проверки

Зависит от `PLAT-2.1`–`PLAT-2.5`.

- `boundary.rs`: `build.rs`, `Cargo.toml`, `examples`, `std::os::*`,
  `RawWindowHandle::*`, `windows_subsystem`, `CARGO_CFG_TARGET_OS`.
- `boundary.rs`: разбирать тестовый модуль, а не обрывать файл на первом `#[cfg(test)]`.
- Разрешать `cfg(windows)` только внутри `#[cfg(test)]`.
- Учесть выросший `app/src/app/ui_tests.rs`: после `bool` → `WindowResult` проверить
  overlay/region snapshots и тесты уведомлений.
- Прогнать `cargo fmt`, Clippy, platform/app tests.

Проверка: полный набор команд из раздела «Проверки».

### PLAT-2.7 — Документация и правило раскладки

Зависит от `PLAT-2.1`–`PLAT-2.6`.

- `specs/architecture.md`, раздел `platform`: описать фактическую раскладку
  `windows/`, общий `screen/capture.rs`, будущие `linux/`/`macos/`/`unix/`.
- `specs/rules.md`: закрепить постоянное правило раскладки платформенного кода.
- `specs/constraints.md`: зафиксировать Windows-only, политику `Unsupported`/`Err` и
  единственную панику — `single_instance` на неизвестной ОС.
- `README.md`: обновить карту крейтов/платформенные заметки, если там видна структура.
- `specs/tasks/platform-layout.md`: записать итог и фактически появившиеся файлы.
- Не дублировать правила в `CLAUDE.md`/`AGENTS.md`; они остаются точками входа.

Проверка: сверить текст с фактическим деревом `crates/platform/src`, проверить ссылки,
убедиться, что `specs/rules.md` не противоречит `specs/architecture.md`.

## Правило раскладки для документации

Это текст, который `PLAT-2.7` должен закрепить в `specs/architecture.md` и
`specs/rules.md`:

- весь проектный платформенно-зависимый код живёт в `mediaa-platform`;
- Windows-реализация — в `crates/platform/src/windows/`;
- `linux/`, `macos/`, `unix/` и подпапки версий Windows/дистрибутивов Linux добавляются
  только вместе с конкретным кодом, а не заранее;
- внешние пакеты, уже абстрагирующие ОС, не переносятся в platform;
- публичный API остаётся доменным; факультативные операции возвращают
  `Unsupported`/`Err`/`None`/`false`, обязательные без реализации паникуют;
- общий `xcap`-код остаётся вне `windows/`, потому что им пользуются и другие ОС.

## Что не делаем

- Не удаляем `x11.rs`, `stream.rs`, unix-`single_instance`, `device_query`, `xcb`, `libc`,
  `cursor_overlay::arrow`, `WindowOpen`, `WindowResult`, `RegionOutline`,
  `cursor::current`.
- Не создаём `linux/`, `macos/`, `unix/`.
- Не трогаем `windows_subsystem`, `winresource`, `ffmpeg/build.rs` и target-секцию
  `tray-icon`.
- Не добавляем CI/Docker под Linux/macOS.

## Проверки

- `cargo check -p mediaa-platform --lib --profile dev-check`;
- `cargo test -p mediaa-platform --lib --test boundary`;
- `cargo clippy -p mediaa-platform --all-targets -- -D warnings`;
- `cargo fmt --check`;
- `cargo check --workspace --lib --bins --profile dev-check`;
- `cargo test -p mediaa --lib` после изменения `bool` → `WindowResult`;
- live-тесты на Windows desktop.

## Риски

- Смена `bool` → `WindowResult` затрагивает `app`; это не удаление фич, но требует правок.
- WGC/GDI-перенос проверять снимком и записью.
- Non-Windows-код остаётся недоделанным; `single_instance` unix по-прежнему содержит
  `super::development_scope()` и, вероятно, не компилируется. Раз Linux/macOS не
  собираются, это допустимо, но при будущей поддержке нужно исправить.
- `dead_code = deny` может потребовать оставить вызовы перенесённых функций.
- Для экрана на неизвестной ОС сообщение об ошибке должно попасть в уведомление; нужен
  тест/проверка, что UI не падает и показывает ошибку.
