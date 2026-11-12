# MediaA 2

Новый workspace MediaA. Архитектура и порядок работ:
[rewrite-crates2.md](../specs/tasks/rewrite-crates2.md).

## Роли

```text
mediaa.exe              launcher
mediaa.exe --backend    трей-демон
mediaa.exe --ui         окно по требованию
mediaa.exe --ui --picker <session>
mediaa.exe --area       выбор области через backend (picker-процесс)
mediaa.exe --fullscreen снимок экрана через backend
```

## Крейты

| Крейт | Ответственность |
|---|---|
| `mediaa2-core` | настройки, форматы, геометрия, библиотечные записи, пути, имена, agent-типы, speakers, chat/transcript/pulse/assistant-типы |
| `mediaa2-i18n` | языки и locale-файлы |
| `mediaa2-protocol` | сообщения, состояние, кадрирование IPC, типизированный протокол транскрибации (PROTOCOL=3) |
| `mediaa2-storage` | только SQL и схема библиотеки |
| `mediaa2-platform` | Windows: WGC, окна (ghost-режим), audio, clipboard, tray, hotkeys, single instance, autostart |
| `mediaa2-media` | recorder, convert, GIF/WebP/FFmpeg, preview, аннотации, screenshot |
| `mediaa2-model-eval` | провайдеры моделей: Engine trait, OpenAI-совместимый/GenAI/Ollama, стриминг, embeddings/rerank, gate/slots, keychain-секреты |
| `mediaa2-agent-transcription` | фича «Транскрибация»: очередь задач с чекпоинтами, стадии (транскрибация/диаризация/голоса/имена/индекс/поиск/темы), ассистент с чатами и веб-поиском, per-media SQLite |
| `mediaa2-backend` | Tokio-демон, сервисы, домены, IPC, сервис Transcription (AgentHost + tick 250 мс) |
| `mediaa2-frontend` | egui-приложение, страницы (включая «Транскрибацию»: очередь, транскрипт, темы, чат с markdown, провайдеры/модели/бюджет), состояние, клиент |
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

- полный workspace: **777 passed** (live- и capture-тесты — ignored, запускаются отдельно);
- реальные smoke-прогоны: снимок экрана, 3-секундная запись видео в MKV,
  3-секундная запись аудио в MP3, конвертация MKV → MP4, холодный запуск лаунчера,
  транскрибация 11.5 мин русского аудио через speech-сервер (GigaAM v3 + pyannote + CAM++, 1065 слов);
- страницы Library/Settings/Capture/Record/Audio/Convert/Transcription работают на реальном `State`;
- трей, хоткеи, single instance, autostart, диалоги, recording/conversion/screenshots
  подключены к backend;
- «Транскрибация»: очередь с прогрессом/отменой, транскрипт со спикерами и
  переименованием, темы, чат с markdown/упоминаниями/стримингом, провайдеры
  (OpenRouter/DeepSeek/HF/Ollama/OpenAI/speech), модели по ролям, бюджеты пулов,
  веб-поиск, Ollama pull/serve — команды через `TranscriptionCommand` (PROTOCOL=3);
- превью: изображения показываются напрямую, для видео/анимаций backend строит
  статические PNG-превью; picker умеет выделение, 8 ручек, Pen/Line/Arrow/Rect/Marker/Text,
  цвет, толщину, undo/redo и выбор региона записи;
- не закончено: hotkey capture, badge-иконки меню трея,
  поиск по индексу в UI (волна 3), перезапуск отдельной стадии задачи.
