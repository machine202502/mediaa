# Чистая граница платформы

## Цель

Убрать все утечки ОС и бэкендов из публичного API `mediaa-platform`. Остальные
крейты должны видеть только доменный контракт: дисплей, окно, аудио-источник,
overlay, буфер обмена, автозапуск. Никаких `Cpal*`, `Wgc*`, `X11*`, `StreamSource`
и OS-модулей снаружи.

## Текущие утечки

1. **`NativeWindow` — Windows-shaped.**
   `from_raw` понимает только `RawWindowHandle::Win32`; `set_visible`/`set_ghost`
   на не-Windows возвращают `false`. App не проверяет ОС, но получает
   Windows-специфичное поведение через «не поддерживается».

2. **`CpalSource` / `CpalTarget` в публичном API и в `app`.**
   `app/recording.rs` создаёт `CpalSource::new(CpalTarget::Input(...))`.
   Это утечка аудиобэкенда, а не ОС, но контракт должен быть доменным.

3. **Публичные OS-модули.**
   `mediaa_platform::wgc`, `mediaa_platform::x11`, `mediaa_platform::stream`
   экспортируются наружу. App их не использует, но они являются публичным API.

4. **Публичные внутренности screen.**
   `MonitorSource`, `WindowSource`, `DeferredSource` и `StreamSource` — публичные
   типы, хотя снаружи нужны только фабрики и `FrameSource`.

5. **Возможности, описанные как bool.**
   `set_visible`/`set_ghost`/`RegionOutline::show` возвращают `bool`, где `false`
   смешивает «не поддерживается» и «не удалось». Для доменного контракта нужен
   явный результат/возможность.

## Целевой публичный API

### screen

Оставить:

- `MonitorInfo`, `WindowInfo`, `MonitorId`, `CaptureWindowId`, `WindowState`,
  `MonitorState`;
- `monitors`, `windows`, `monitor_state`, `window_status`;
- `monitor_source`, `window_source_checked`, `deferred_*`, возвращающие
  `SourceFactory`/`Box<dyn FrameSource>`;
- `MonitorSource`/`WindowSource` можно оставить только если они реально нужны
  снаружи; иначе `pub(crate)`.

Убрать из публичного API:

- `wgc`, `x11`, `stream` — `pub(crate)`;
- `StreamSource`, `WgcSource`, `X11Source`;
- внутренние OS-типы и функции.

### audio

Вместо `CpalSource`/`CpalTarget`:

- `audio::input_source(name)` и `audio::system_source()`, возвращающие
  `anyhow::Result<Box<dyn AudioSource>>`;
- `AudioOut` остаётся;
- `input_devices`, `default_input_device`, `system_audio_supported` остаются
  доменными.

CPAL полностью внутри `mediaa-platform::audio`; публичного `AudioInput` не вводим,
потому что app нужен только готовый `AudioSource`, а не описание выбора.

### window

- `NativeWindow` остаётся публичным, но только как непрозрачный handle.
- `from_raw(RawWindowHandle)` внутри разбирает все поддерживаемые варианты
  (Win32/AppKit/Xlib/Wayland), а не только Win32.
- `set_visible`/`set_ghost` возвращают
  `WindowResult { Ok, Unsupported, Failed }`, а не двусмысленный `bool`.
- Никаких `HWND`/`XID`/`NSWindow` в публичных сигнатурах.

### outline, keys, startup, single_instance, files, clipboard

- Публичные функции и типы остаются доменными.
- `RegionOutlineBackend` и `with_backend` уходят из публичного API; тестовая
  инъекция — через `#[cfg(test)]` внутри модуля.
- `print_screen_down` остаётся доменным `bool`.

## Правила зависимостей

- `windows-sys`, `windows-capture`, `xcap`, `cpal`, `x11rb`, `objc2` — только в
  `crates/platform/Cargo.toml`.
- `#[cfg(windows/linux/macos)]` — только в `crates/platform`, кроме тестов,
  примеров и `cfg(test)`-блоков.
- Ни один крейт, кроме `mediaa-platform`, не импортирует `mediaa_platform::wgc`,
  `::x11`, `::stream` или `Cpal*`.
- `mediaa-media` не зависит от `mediaa-platform`; это сохраняется.

## Как предотвратить регрессию

Добавить архитектурный тест/CI-проверку:

- сканирует `crates/*/src`, кроме `crates/platform`, и падает на
  `cfg(windows)`, `cfg(target_os`, `windows_sys`, `xcap`, `cpal`, `x11rb`,
  `mediaa_platform::{wgc,x11,stream}`, `CpalSource`, `CpalTarget`;
- разрешает эти строки только в `#[cfg(test)]`-модулях и examples;
- проверяет, что `mediaa-media` не имеет зависимости `mediaa-platform`;
- проверяет публичный API `mediaa-platform` на отсутствие OS-имён (doc/compile
  tests).

## Тесты

- `mediaa-platform` собирается и проходит unit-тесты.
- `app` использует только доменные фасады: `audio::input/system`, `screen::*`,
  `window::NativeWindow`, `outline::RegionOutline`.
- Тест-заглушки `FrameSource`/`AudioSource` продолжают работать.
- Архитектурный тест падает, если вне platform появился OS-зависимый импорт.
- UI-тесты не меняются по поведению; меняются только импорты.
- Linux/macOS-поведение `NativeWindow` возвращает явный `Unsupported`, а не
  молчаливый `None`/`false`.

## Решения

1. `set_visible`/`set_ghost` возвращают `WindowResult { Ok, Unsupported, Failed }`.
2. `NativeWindow` сейчас явно возвращает `Unsupported` для не-Win32; полная
   реализация AppKit/Xlib/Wayland — отдельная задача.
3. Аудио: функции `audio::input_source(name)` и `audio::system_source()`,
   возвращающие `Box<dyn AudioSource>`. Публичного `AudioInput` не вводим.
4. `RegionOutlineBackend` скрываем; тестовая инъекция — через `#[cfg(test)]` внутри
   модуля. Снаружи только `RegionOutline`.

## Статус

План зафиксирован, решения приняты, реализация не начата. Задача — предотвратить
утечки до добавления Linux/macOS-бэкендов.
