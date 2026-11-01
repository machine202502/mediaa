# Архитектура MediaA

MediaA находится в active development и ещё не имеет released versions. Документ описывает
текущую систему и границы ответственности. Его нужно обновлять при изменении кода; исторические
планы здесь не хранятся.

## 1. `core` — общие контракты

Крейт не зависит от других workspace-крейтов. Он содержит общие типы и правила, которые нужны
нескольким слоям приложения.

### 1.1. `settings`

Хранит persisted-настройки, профили записи, выбор устройств и sanitization. Effective FPS/height
записи остаются в `RecordSettings`; animation quality хранит только GIF/WebP quality.
Пользовательский файл настроек и каталоги данных описаны в разделе 7.7.

### 1.2. `formats`, `frame`, `geometry`

Определяют форматы медиа, представление кадра, прямоугольники, selection и resize geometry.

### 1.3. `source`

Содержит `FrameSource`, `AudioSource` и `SourceFactory`. Эти контракты отделяют recording от OS
capture и позволяют использовать синтетические источники в тестах.

### 1.4. `pacer` и `audio`

`pacer` управляет временными слотами кадров и активным временем записи. `audio` содержит общие
аудио-типы, таймлайн и укладку.

### 1.5. `paths`, `naming`, `atomic_file`

Определяют пользовательские и рабочие каталоги, имена результатов и атомарную публикацию файлов
без перезаписи существующего результата.

### 1.6. `library` и `limits`

Содержат общие описания библиотечных записей, фильтры, группировку, форматирование и пределы
настроек.

### 1.7. `dev`, `hash`, `logging`

Изолируют dev overrides, стабильное хэширование путей и начальную настройку логирования.

### 1.8. `agent`, `catalogs`, `chat`, `outline`, `pulse`, `retrieval`, `speakers`, `transcript`, `slots`

Определяют настройки агента, каталоги записей, историю чатов, план и разбор записи,
метрику загрузки моделей, поисковый индекс, имена говорящих и расшифровку записи. `slots`
держит очереди слотов пулов провайдеров и инструмента поиска: лимиты и привязку подключений
берёт из `AgentSettings`, выдаёт слот по FIFO.

## 2. `platform` — операционная система

Крейт зависит от `core`. Он отвечает за native API и платформенные различия. Сейчас
поддерживается и проверяется только Windows; Linux/macOS-код остаётся в дереве, но не
считается поддержанным.

### 2.1. Раскладка

```text
crates/platform/src/
  screen/
    mod.rs       # доменный фасад
    types.rs     # MonitorInfo, WindowInfo, id и состояния
    capture.rs   # общий xcap-захват, источники, DeferredSource
  windows/
    screen.rs    # WGC и Win32-состояние окна
    wgc.rs
    window.rs    # NativeWindow, ghost, capture exclusion, прозрачность, анимация
    outline.rs   # WindowsBackend
    outline_window.rs
    cursor.rs    # GDI-курсор
    keys.rs
    single_instance.rs
    startup.rs
    audio.rs
    init.rs      # DPI awareness
  window.rs      # публичные WindowOpen/WindowResult/RegionOutline
  outline.rs     # публичный OutlineGeometry/RegionOutline
  cursor.rs      # CursorState/position/current
  keys.rs
  single_instance.rs
  startup.rs
  audio.rs
  clipboard.rs
  files.rs
  cursor_overlay.rs
  linux/
    mod.rs
    x11.rs       # существующий X11
  unix/
    mod.rs
    stream.rs    # существующий Linux/macOS capture
    single_instance.rs
    cursor.rs
```

Весь проектный Windows-код лежит в `windows/`; существующий Linux/X11 — в `linux/`;
общий POSIX-код Linux/macOS — в `unix/`. `macos/` появится, когда появится отдельный
macOS-only код. Общий `xcap`-код остаётся в `screen/capture.rs`, потому что им пользуются
и Windows, и сохранённые x11/stream-пути. Внешние пакеты, уже абстрагирующие ОС (`xcap`,
`cpal`, `clipboard-rs`, `opener`, `trash`, `auto-launch`), в папки ОС не переносятся.

### 2.2. `screen`

`screen/mod.rs` — доменный фасад. `screen/types.rs` содержит `MonitorInfo`, `WindowInfo`,
`MonitorId`, `CaptureWindowId`, `WindowState`. `screen/capture.rs` перечисляет мониторы и
окна, снимает кадр через `xcap`, держит `MonitorSource`/`WindowSource`/`DeferredSource` и
общий пересчёт координат. WGC-путь, проверки Win32-окна и screenshot через WGC — в
`windows/screen.rs`.

На неподдерживаемой ОС `monitors`/`windows`/`capture_*`/`*_source` возвращают `Err`
«screen capture is not supported on this platform»; `monitor_at` → `None`,
`monitor_state` → `Disconnected`, `window_status` → `Missing`. Приложение показывает
ошибку через существующие уведомления.

### 2.3. `window`, `outline`, `cursor`, `cursor_overlay`

`window.rs` хранит непрозрачный `NativeWindow`, `WindowOpen`, `WindowResult` и приватный
`RegionOutline`. Win32-тела, capture exclusion, прозрачность, анимация открытия и native
layered-окно контура — в `windows/window.rs`, `windows/outline.rs`,
`windows/outline_window.rs`.

`outline::RegionOutline::show` и window effects возвращают `WindowResult`:
`Ok` — выполнено, `Unsupported` — ОС/возможность недоступна, `Failed` — Win32-вызов не
удался. `NativeWindow::from_raw` на не-Win32 возвращает `WindowOpen::Unsupported`.
`cursor::current`/`position` сохраняют `Option`-семантику; GDI-курсор — в
`windows/cursor.rs`, non-Windows `device_query` — в `cursor.rs`.

### 2.4. `audio`, `clipboard`, `files`

`audio` — cpal-фасад: микрофоны, system audio, `AudioOut` и confirmation tone.
`system_audio_supported` — `true` в `windows/audio.rs`, `false` на остальных.
`clipboard` и `files` — обёртки над внешними `clipboard-rs`, `opener`, `trash`; они не
содержат проектного платформенного кода и остаются на верхнем уровне.

### 2.5. `single_instance`, `startup`, `keys`

`single_instance` — фасад над Win32 mutex/event (`windows/single_instance.rs`) и unix
flock/socket (`single_instance.rs`); на неизвестной ОС `acquire` паникует.
`startup` — auto-launch; Windows-цитирование пути — в `windows/startup.rs`.
`keys::print_screen_down` — Win32-опрос в `windows/keys.rs`, `false` на остальных.

### 2.6. Правило раскладки

- Весь проектный платформенно-зависимый код живёт в `mediaa-platform`.
- Windows-реализация — в `crates/platform/src/windows/`.
- `linux/`, `macos/`, `unix/` содержат существующий код; новые подпапки версий
  Windows/дистрибутивов Linux добавляются только вместе с конкретным кодом, а не заранее.
- Внешние пакеты, уже абстрагирующие ОС, не переносятся в platform.
- Публичный API остаётся доменным. Факультативные операции возвращают
  `Unsupported`/`Err`/`None`/`false`; обязательные без реализации паникуют.
- `cfg(windows)` в тестах допустим только внутри `#[cfg(test)]`.

## 3. `media` — медиа-операции

Крейт зависит от `core`, `mediaa-ffmpeg`, `mediaa-webp` и `mediaa-gif`. Он не знает
о страницах UI.

### 3.1. `recorder`

Владеет recording session, stop/finalize, recovery и source lifecycle.

#### 3.1.1. `recorder/request`

Валидирует `RecordingRequest`, профиль записи и limits.

#### 3.1.2. `recorder/capture`

Запускает и останавливает источники, ожидает readiness и собирает captured работы.

#### 3.1.3. `recorder/finalize`

Преобразует исходники в итоговые файлы, связывает аудио и выполняет recovery.

#### 3.1.4. `recorder/recovery`

Восстанавливает незавершённые пользовательские источники после сбоя.

### 3.2. `animation_backends`

`animation_backends` только выбирает форматный крейт: GIF кодирует `mediaa-gif`, WebP —
`mediaa-webp`. Форматные крейты владеют графом, output-опциями и декодированием WebP; `media`
не строит для анимации собственный FFmpeg job. Animated WebP в GIF идёт через поток кадров
`mediaa-webp`, потому что FFmpeg декодирует только статичный кадр WebP.
`WebpMode::{Quality, Fast}` задаёт effort. Сегментного и live-WebP больше нет.

### 3.3. `convert`, `filters`

`convert` собирает FFmpeg job через `mediaa-ffmpeg` и выполняет пользовательские конвертации.
`filters` содержит только строки фильтров для видео, аудио и preview.

### 3.4. `info`, `preview`, `preview_cache`, `thumbnail`

Определяют медиа metadata, hover-preview, proxy и thumbnail operations.

### 3.5. `error`

`error` классифицирует ошибки, от которых зависит пользовательское действие или recovery.
FFmpeg execution/cancel живёт в отдельном `mediaa-ffmpeg`.

## 4. Форматные крейты

- `mediaa-ffmpeg` — единственный владелец `ez-ffmpeg`: job builder/start/wait, probe,
  frame export, push-источник, PCM-декодирование, capabilities и отмена.
- `mediaa-webp` — WebP-граф, декодер (`Reader`), поток кадров, кодирование,
  normalize/validate/trim/merge animated WebP.
- `mediaa-gif` — GIF-граф, кодирование и trim.

`mediaa-media` не импортирует `ez-ffmpeg` или `image-webp`; форматные операции вызываются
через эти крейты.

## 5. `storage` — библиотека и кэши

Крейт зависит от `core`.

### 5.1. `library_index`

Сканирует каталог, запрашивает/сортирует записи, обновляет details и следит за изменениями.
Watcher overflow требует reconciliation.

### 5.2. `cache_db`

Содержит SQLite schema и background persistence индекса.

### 5.3. `thumb_cache`

Хранит thumbnail atlas и перестраиваемые thumbnail slots.

### 5.4. `screenshot`

Сохраняет кадр в выбранный image format.

## 6. `render`

Крейт зависит от `core`.

### 6.1. `annotate`

Рисует аннотации и кропает итоговый screenshot.

### 6.2. `layout`

Размещает selection, tools и actions внутри границ display.

## 7. `app` — desktop application

Крейт зависит от остальных workspace-крейтов по реальной необходимости.

### 7.1. Shell

#### 7.1.1. `app.rs`

Создаёт приложение, связывает владельцев, навигацию, settings, tray/hotkey/window lifecycle и
выполняет polling независимо от видимой страницы.

#### 7.1.2. `app/actions`

Обрабатывает межфункциональные действия, exit и tray state.

#### 7.1.3. `app/state`, `app/command`

Содержат Page, pending action и общие shell-команды. Feature business state здесь не хранится.

#### 7.1.4. `app/desktop`, `app/recovery`

Владеют open/reveal/rename и startup recovery как app-level effects.

### 7.2. User features

#### 7.2.1. `features/recording`

Владеет выбранным target, `Idle/Starting/Recording/Saving`, session и finalizer.
`Saving` не освобождает результат до завершения cleanup. Завершённая запись индексируется в
библиотеке и копируется в буфер как файл.

#### 7.2.2. `features/capture`

Владеет capture/screenshot jobs и overlay. UI возвращает действия, а window visibility остаётся
ответственностью shell.

#### 7.2.3. `features/conversion`

Владеет input/options, probe, estimate, conversion job и last output. Estimate заменяем и
защищён generation.

#### 7.2.4. `features/library`

Владеет индексом, watcher, scan/reconciliation, query/hits, removal, previews и viewer.

##### `features/library/viewer` и `player`

Показывают выбранный файл, видео и audio playback.

##### `features/library/hover_preview` и `thumbs`

Управляют hover media и thumbnail atlas.

##### `features/library/cleanup`

Содержит одну removal operation с `Selected` и `OlderThan`; bulk age перепроверяется.

#### 7.2.5. `features/agent`

Владеет подключениями, слотами и scheduler очереди. Хост запускается, когда открыт раздел
Agent или в очереди остались незавершённые задачи; изменение лимитов параллелизма обновляет
slots живого scheduler, а не пересоздаёт его. Каждый вызов модели идёт через `Gated`: перед
вызовом он берёт слот группы подключения, если группа задана. Пулы очереди постоянно большие
и не дублируют лимиты групп. Ключи подключений загружаются и сохраняются через
`agent/secrets` (OS keychain), а не через `core`. Инференса внутри приложения нет.

#### 7.2.6. `pages/agent`

Рисует UI агента: каталоги, подключения, модели, очередь и лимиты параллелизма. Страницы находятся в
`pages/agent`, навигация — через `Section::Agent`, вкладки студии — в
`pages/agent/tab_strip.rs`. Каталоги рисуют собственный split-view в `catalogs::screen`;
shell не содержит их layout. Фоновые проверки, загрузки и поиск используют общие
`jobs::Job` и `latest_worker::LatestWorker`.

### 7.3. Pages

`pages/record`, `pages/audio`, `pages/capture`, `pages/settings`, `pages/about` рисуют конкретные
экраны и возвращают локальные действия. Страницы не владеют background lifecycle.

### 7.4. Services

`services/clipboard` выполняет native clipboard worker, timeout и recovery.
`services/settings_writer` объединяет частые изменения настроек и пишет последнюю версию вне UI.

### 7.5. Shared app mechanisms

- `jobs.rs` — one-shot `Job<T>` с owned handle и однократным poll.
- `latest_worker.rs` — bounded replacement worker для устаревающих запросов.
- `app/notices.rs` — уведомления, fade lifecycle, auto-hide, hover pause и silent actions.
- `tray.rs`, `hotkeys.rs` — desktop adapters.
- `themes/`, `widgets/`, `icon.rs`, `i18n.rs` — presentation and localization.
- `overlay.rs` — screenshot editor. Selection определяет итоговый crop, но annotations хранятся в
  координатах полного кадра и видны во всём editor viewport. Активный text editor имеет рамку,
  перетаскивается за её поля и фиксируется кликом снаружи. Overlay использует один native viewport;
  opaque viewport создаётся скрытым заранее и переиспользуется при открытии; `Tab` меняет monitor
  frame через один capture job без создания нового окна. Сохранение области идёт через системный
  диалог; формат берётся из расширения, а overlay закрывается только после успешной записи.
  Перед снимком области окно скрывается и выдерживается `HIDE_DELAY`, затем показывается overlay.

### 7.6. Reusable widgets and themes

Переиспользуемые UI-примитивы живут только в `crates/app/src/widgets/`, по одному виджету на
файл:

```text
crates/app/src/widgets/
  mod.rs
  button.rs
  icon_button.rs
  layout.rs
  menu_item.rs
  modal.rs
  segmented.rs
  track_slider.rs
```

`widgets/mod.rs` объявляет модули и реэкспортирует публичные функции. Страницы и features
обязаны использовать эти примитивы вместо локальных копий кнопок, иконок, модалок, сегментов и
слайдеров. В `pages/` и `features/` остаётся только композиция конкретного экрана и его
бизнес-состояние; новый общий виджет добавляется отдельным файлом в `widgets/`.

Темы и их палитры живут в `crates/app/src/themes/`:

```text
crates/app/src/themes/
  mod.rs
  dark.rs
  light.rs
  lime.rs
  peach.rs
```

`themes/mod.rs` владеет контрактами `Palette` и `IconPalette`, выбором текущей палитры,
применением темы и общими theme-хелперами. Каждый файл темы хранит цвета своей палитры и
градиент иконки MediaA. Новая тема добавляется отдельным файлом и реэкспортом в `mod.rs`.
Страницы не содержат собственных цветовых констант: цвета берутся из текущей палитры.

### 7.7. User settings and data locations

Основной файл пользовательских настроек:

```text
~/.mediaa.toml
```

Путь задаёт `core::paths::settings_file()`; `~` — домашний каталог пользователя из
`directories::UserDirs`. Если файла нет, `Settings::load()` ищет legacy-файл
`ProjectDirs::from("dev", "MediaA", "MediaA").config_dir()/settings.toml`, загружает его и
сохраняет настройки уже в `~/.mediaa.toml`. В TOML хранятся все пользовательские настройки,
профили записи и тема; ключи провайдеров хранятся отдельно в OS keychain (`agent/secrets`).

Запись выполняет `services/settings_writer`: изменения объединяются, последняя версия
сохраняется вне UI, файл публикуется атомарно. Каталоги данных задаёт
`core::paths::data_dir()` через `ProjectDirs::data_local_dir()` (fallback —
`std::env::temp_dir()/MediaA`): `logs/` — логи, `work/` — рабочие файлы незавершённых операций. Медиатека по
умолчанию находится в `Documents/MediaA`, а не в каталоге настроек.

## 8. `agent`, `engine`, `queue`, `eval` — агентские крейты

`queue` зависит только от внешних крейтов и хранит задачи, композиты, пулы ресурсов и
chunked runs. `engine` зависит от `core`. Чат и эмбеддинги идут через `genai`, у каждого
подключения свой клиент и свой прокси. Расшифровка, реранк, список моделей и pull Ollama
остаются своим HTTP. `Gated` — обёртка `Engine`, которая берёт слот группы подключения перед
каждым запросом к модели. Спикеры и отпечаток приходят в ответе расшифровки, отдельной роли и
стадии очереди у них нет. Своих моделей и движков инференса нет. `agent` зависит от
`core`, `media`, `engine` и `queue` и собирает chat loop, инструменты, индексацию и расшифровку;
секреты подключений хранятся в OS keychain (`agent/secrets`). `eval` зависит от агентских
крейтов и прогоняет оценку качества.

## 9. Языковые файлы

Переводы:

```text
crates/app/locales/en.toml
crates/app/locales/ru.toml
```

Загрузка, fallback и список языков:

```text
crates/app/src/i18n.rs
```

Locale-ключи должны совпадать во всех языковых файлах. Тесты проверяют отсутствие неизвестных
ключей в коде и расхождений между locale.

## 10. Зависимости

```text
core
  <- render
  <- storage
  <- platform
  <- mediaa-ffmpeg
  <- mediaa-webp <- mediaa-ffmpeg
  <- mediaa-gif <- mediaa-ffmpeg
  <- media <- mediaa-ffmpeg, mediaa-webp, mediaa-gif
  <- engine
  <- queue
  <- agent <- engine, queue
  <- eval <- agent, engine, queue
  <- app <- все перечисленные, кроме eval
```

`core` — общий фундамент, а не строго чистая domain model. Новые OS API и FFmpeg зависимости в
него не добавляются.
