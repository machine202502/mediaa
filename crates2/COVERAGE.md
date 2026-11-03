# MediaA 2: запуск и покрытие

## Запуск из корня проекта

```powershell
cd C:\shared-data\reps\.old\gifcap2
.\crates2\target\debug\mediaa.exe
```

Перед запуском закрой старый MediaA: у приложения один экземпляр на пользователя.

Роли:

```powershell
.\crates2\target\debug\mediaa.exe --tray   # только backend + трей
.\crates2\target\debug\mediaa.exe --ui     # только окно, если backend уже запущен
```

Красная dev-копия:

```powershell
.\crates2\target\debug\mediaa-agent.exe
```

## Изолированный запуск

Не трогает рабочие настройки и библиотеку:

```powershell
$root = Join-Path $PWD 'crates2\target\manual'
New-Item -ItemType Directory -Force $root,(Join-Path $root 'library'),(Join-Path $root 'data') | Out-Null
$env:MEDIAA2_INSTANCE_ID = 'manual'
$env:MEDIAA2_SETTINGS = Join-Path $root 'settings.toml'
$env:MEDIAA2_LIBRARY = Join-Path $root 'library'
$env:MEDIAA2_DATA_DIR = Join-Path $root 'data'
.\crates2\target\debug\mediaa.exe
```

Для красной dev-копии в изолированном окружении:

```powershell
$env:MEDIAA2_DEV_AGENT = '1'
.\crates2\target\debug\mediaa.exe
```

## Автоматические проверки

Полный workspace: **437 passed, 1 ignored**.

| Крейт | Тесты | Что покрыто |
|---|---:|---|
| backend | 142 (+1 ignored) | settings, library query/rename/delete/cleanup, IPC handshake/state/quit, recording domains/plan/recovery, conversion queue/cancel, screenshots/picker, presence/tray/hotkeys/autostart |
| core | 105 | settings, formats, geometry, query, recording, annotate, pacer, paths, naming, notices, navigation, frame, hash |
| frontend | 72 | все основные страницы в dark/light/narrow, модалки, меню Library, picker, viewer, convert statuses |
| media | 61 | FFmpeg convert, GIF/WebP, filters, annotate, screenshot compose, preview cache, recorder |
| platform | 35 | tray menu/state/progress, hotkeys parsing, single instance, startup, screen, files, audio |
| protocol | 12 | JSON roundtrip всех основных сообщений, wire framing/limits |
| storage | 7 | schema, query/sort/pagination, corrupt rows, foreign schema |
| i18n | 3 | en/ru key parity, language resolution |

Плюс ручные end-to-end прогоны на изолированных каталогах:

- снимок экрана — PNG ~1.3 MB;
- 3-секундная запись видео — MKV ~174 KB;
- 3-секундная запись аудио — MP3 ~99 KB;
- конвертация MKV → MP4 — ~166 KB;
- холодный запуск лаунчера — backend + UI + library.db;
- второй UI — завершается, backend остаётся.

## Что посмотреть руками

1. **Трей**: иконка, меню, открытие окна, старт записи, quit.
2. **Screenshot**: Select area, выделение, ручки, Pen/Line/Arrow/Rect/Marker/Text, Copy/Save/Cancel.
3. **Record**: Screen/Windows/Region, нижний Start bar, status overlay, pause/stop/cancel, сохранение.
4. **Audio**: MP3/формат/битрейт, микрофон/системный звук, нижний Start bar, progress overlay.
5. **Convert**: несколько файлов, успех/ошибка/отмена, галочки/крестики, retry, Cancel.
6. **Library**: поиск/фильтр/сортировка, выделение, rename/delete, cleanup week/30 days, hover-видео, viewer, light theme.
7. **Settings**: simple/advanced, выключение advanced с confirm, reset, autostart, hotkeys, папки.
8. **Темы**: dark/light, светлый picker и viewer.
9. **Закрытие**: close-to-tray, повторный запуск, quit.

## Известные ограничения

- Viewer показывает chrome и изображение, но видео/аудио открывает системным плеером; встроенного playback/seek пока нет.
- Статусы Convert: Done берётся из `done`, Failed — из notice; отменённый файл показывается прочерком. Структурированного per-file результата в protocol пока нет.
- Picker-снепшоты используют синтетический кадр; реальный захват экрана тестами не запускается.
- Hotkey capture — на уровне egui-событий; конфликт проверяется нормализованными строками, не парсером слотов.
- В Windows 11 новая tray-иконка может попасть в overflow — её нужно один раз вытащить в панель.
- По сырому числу тестов новый workspace меньше старого: старый содержит ~1101 `#[test]`, включая агент и legacy-снепшоты. Новые 437 сфокусированы на текущих путях и дополнены реальными e2e-прогонами.
