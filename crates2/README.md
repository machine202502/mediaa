# MediaA 2

Новый workspace MediaA. Архитектура и порядок работ:
[rewrite-crates2.md](../specs/tasks/rewrite-crates2.md).

## Роли

```text
mediaa.exe              launcher
mediaa.exe --backend    трей-демон
mediaa.exe --ui         окно по требованию
mediaa.exe --ui --picker <session>
```

## Крейты

| Крейт | Ответственность |
|---|---|
| `mediaa2-core` | настройки, форматы, геометрия, библиотечные записи, пути, имена |
| `mediaa2-i18n` | языки и locale-файлы |
| `mediaa2-protocol` | сообщения, состояние, кадрирование IPC |
| `mediaa2-storage` | только SQL и схема библиотеки |
| `mediaa2-platform` | Windows: WGC, окна, audio, clipboard, tray, hotkeys, single instance, autostart |
| `mediaa2-media` | recorder, convert, GIF/WebP/FFmpeg, preview, аннотации, screenshot |
| `mediaa2-backend` | Tokio-демон, сервисы, домены, IPC |
| `mediaa2-frontend` | egui-приложение, страницы, состояние, клиент |
| `mediaa2` | лаунчер и роли |

## Сборка и проверки

```powershell
$env:FFMPEG_DIR = (Resolve-Path vcpkg_installed/x64-windows-static-release).Path
cargo build --manifest-path crates2/Cargo.toml -p mediaa2
cargo test  --manifest-path crates2/Cargo.toml --workspace
cargo clippy --manifest-path crates2/Cargo.toml --workspace --all-targets -- -D warnings
```

FFmpeg/vcpkg — из корневого `scripts/windows/build/env.ps1`.

## Изоляция при проверках

Не трогать пользовательский экземпляр:

```powershell
$env:MEDIAA2_INSTANCE_ID = 'test'
$env:MEDIAA2_SETTINGS     = '...\settings.toml'
$env:MEDIAA2_LIBRARY      = '...\library'
$env:MEDIAA2_DATA_DIR     = '...\data'
```

`MEDIAA2_DEV_BACKGROUND=1` ставит окно за экран, `MEDIAA2_DEV_CAPTURE_UI=1`
разрешает снимать это окно скриптом проверки. Для smoke-сценариев:
`MEDIAA2_DEV_ACTION=shot|record|audio|convert` (convert читает `MEDIAA2_DEV_INPUT`).
`mediaa-agent.exe` и `MEDIAA2_DEV_AGENT=1` включают красную иконку dev-копии.

## Дизайн-снепшоты

Страницы рендерятся без запуска приложения через `egui_kittest`:

```powershell
$env:UPDATE_SNAPSHOTS='1'
cargo test --manifest-path crates2/Cargo.toml -p mediaa2-frontend design_
python crates2/scripts/design_sheet.py
```

PNG лежат в `crates2/crates/frontend/tests/snapshots`, контактный лист —
`crates2/target/design-contact-sheet.png`. Для параллельной работы агентов
используются отдельные `CARGO_TARGET_DIR`: `target-design`, `target-library`,
`target-settings`, `target-capture`, `target-convert`.

## Текущее состояние

- полный workspace: 382 passed, 1 ignored (реальный захват рабочего стола);
- реальные smoke-прогоны: снимок экрана, 3-секундная запись видео в MKV,
  3-секундная запись аудио в MP3, конвертация MKV → MP4, холодный запуск лаунчера;
- страницы Library/Settings/Capture/Record/Audio/Convert работают на реальном `State`;
- трей, хоткеи, single instance, autostart, диалоги, recording/conversion/screenshots
  подключены к backend;
- превью: изображения показываются напрямую, для видео/анимаций backend строит
  статические PNG-превью; picker умеет выделение, 8 ручек, Pen/Line/Arrow/Rect/Marker/Text,
  цвет, толщину, undo/redo и выбор региона записи;
- не закончено: видеопросмотр, hotkey capture, badge-иконки меню трея, page-level kittest.
