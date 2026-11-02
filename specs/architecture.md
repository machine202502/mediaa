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

Слои внутри крейта: `ui` (отрисовка), `app` (composition root), `features`
(состояние фич и use cases), `infra` (ОС и транспортные швы). Зависимости идут
`ui → app → features → infra`; `features` не знает про `ui`/`app`, `infra` не
знает про `ui`/`features`. Проверка — `boundaries`.

### 7.1. App

#### 7.1.1. `app.rs`

Создаёт приложение, связывает владельцев, навигацию, settings, tray/hotkey/window lifecycle и
выполняет polling независимо от видимой страницы. Видимость окна, «призрачность» и тихий режим
уведомлений хранит `features/window_presence.rs`: один владелец вместо трёх отдельных флагов.

#### 7.1.2. `app/actions`

Обрабатывает межфункциональные действия, exit и tray state.

#### 7.1.3. `app/state`, `app/command`

Содержат Page, pending action и общие app-команды. Feature business state здесь не хранится.

#### 7.1.4. `app/desktop`, `app/recovery`

Владеют open/reveal/rename и startup recovery как app-level effects.

### 7.2. User features

#### 7.2.1. `features/recording`

Владеет выбранным target, `Idle/Starting/Recording/Saving`, session и finalizer.
`Saving` не освобождает результат до завершения cleanup. Завершённая запись индексируется в
библиотеке и копируется в буфер как файл.

Планирование записи — чистая функция `features/recording.rs::plan`: проверка доступности цели,
профиль, источники экрана и окна, разрешение микрофона и системного звука, имя рабочего каталога
и сборка `RecordingRequest`. Ошибки приходят типом (`Unavailable`, `NoAudioSource`, `Request`),
а тексты уведомлений и показ окна остаются в app.

#### 7.2.2. `features/capture`

Владеет job-обёрткой capture/screenshot и `capture_client`: отдельным соединением
`CLIENT_CAPTURE` к backend. Кадр снимает и итоговый файл пишет backend; frontend получает
`CaptureSourceWire` с backend-issued путём, компонует кадр через `mediaa_render`, кладёт его
в этот путь и отправляет `screenshot_commit`. UI возвращает действия, а window visibility и
само окно оверлея остаются ответственностью app: `App.overlay` держит `Overlay`,
контроллер о нём не знает. Что делать со снимком (сохранить файл, положить изображение в
буфер, положить файл в буфер, формат и путь) решает `features/screenshot.rs::plan`; commit
выполняет backend, app показывает уведомления.

#### 7.2.3. `features/conversion`

Владеет input/options, estimate и last output. Probe идёт через backend
(`agent_request` kind `media_probe`), conversion — через `conversion_*` команды; Estimate
заменяем и защищён generation. Отрисовка страницы — `ui/pages/convert/controller.rs` и
`ui/pages/convert/controls.rs`; системный выбор файлов и папки — `infra/dialogs`.

#### 7.2.4. `features/library` и `ui/pages/library`

Владеет индексом, watcher, scan/reconciliation, query/hits, removal и viewer:
  состояние выделения, индекс, hits, jobs сканирования и удаления. Отрисовка и текстурные кэши живут
  в `ui/pages/library/`: `controller.rs` (страница, тулбар, сетка, bulk-панель), `actions.rs`
  (модалки удаления, переименования и очистки), `grid.rs` (плитки и тулбар), `viewer.rs` (плеер и
  `ViewerTexture`), `thumbs.rs` (атлас миниатюр `Thumbnails`), `hover_preview` (кадры
  `Previews`/`HoverPreview`). Trash/delete и cleanup уходят backend-командой `library_remove`;
  миниатюры, hover- и playback-прокси готовит backend по protocol (`PreviewKindWire`), UI
  запрашивает путь и создаёт `TextureHandle`. Rename файла пока остаётся в app.
  Владение состоянием осталось у контроллера: UI получает его `&mut` и возвращает действия.

##### `features/library/viewer` и `player`

`features/library/viewer.rs` держит воспроизведение: открытие файла, jobs загрузки кадра и звука,
зум, пауза, `Drop`. Отрисовка плеера (`show`, media area, top bar, controls, timeline) — в
`ui/pages/library/viewer.rs` как `impl Viewer`: код в UI, состояние и владение — в features.
`player` остаётся владельцем воспроизведения видео и звука.

##### `features/library/hover_preview` и `thumbs`

Управляют hover media и thumbnail atlas.

##### `features/library/cleanup`

Содержит одну removal operation с `Selected` и `OlderThan`; bulk age перепроверяется.

#### 7.2.5. Агент: только backend

`AgentHost` живёт в backend-процессе (`crates/backend/src/features/agent/host.rs`) и владеет
подключениями, слотами, scheduler, store, streams и feed. Хост тикает всегда, пока backend
жив, независимо от того, открыт ли UI. Изменение лимитов обновляет slots живого scheduler, а
не пересоздаёт его. Каждый вызов модели идёт через `Gated`. Ключи подключений загружаются и
сохраняются через `agent/secrets` (OS keychain), а не через `core`.

Во frontend нет `AgentServices`, `Scheduler`, `Store`, `Streams`, `Feed` или `Engine`.
`features/agent` хранит только view-модели, конвертацию wire-типов и общий request buffer;
`ui/pages/agent` читает `AgentSnapshotWire`, live-события и отправляет команды.

#### 7.2.6. `ui/pages/agent`

Рисует UI агента: каталоги, подключения, модели, очередь и лимиты параллелизма. Страницы находятся в
`ui/pages/agent`, навигация — через `Section::Agent`, вкладки студии — в
`ui/pages/agent/tab_strip.rs`. Каталоги рисуют собственный split-view в `catalogs::screen`;
app не содержит их layout. Очередь, чат, models/providers/budget и поиск работают только с
protocol: snapshot, `AgentEventWire`, `agent_op`/`agent_task`/`agent_steer`/`agent_analyse`/
`agent_names` и `agent_request`. Async-ответы складываются в общий `features::agent::requests`
буфер и забираются страницами по id.

Системные эффекты этих страниц (диалоги, reveal, чтение лога, звук, буфер) идут через
`infra/`, а не напрямую из UI.

### 7.3. Pages

Страницы `ui/pages/*` рисуют экраны и не знают про `app`: они получают
`ui/pages/ctx.rs::Ctx` — срез состояния приложения на кадр (`settings`, `lib`, `recording`,
`capture`, `conversion`, `agent_ui`, текстуры библиотеки, `notices`, каталоги
мониторов, окон и микрофонов). Тип `Ctx` принадлежит `ui`, поэтому `ui` не импортирует
`crate::app` — это проверяет тест границ.

Изменения состояния страница применяет к своим срезам, а эффекты отдаёт наружу: либо
`Vec<Intent>` (уведомление, открытие пути, reveal, переименование, конвертация, команды записи,
hotkeys, preview-кэш), либо `AppCommand`. `App::frame_ui` собирает `Ctx`, вызывает
`ui::pages::frame`, забирает `intents` и выполняет их через `App::handle_intents`. Навигация
(`page`, `page_scroll`, `last_capture_page`, `last_main_page`, `last_agent_page`) хранится в `Ctx`
значениями и копируется обратно после кадра. Background lifecycle страницам не принадлежит.

### 7.4. Services и infra (слой `infra`)

`infra/clipboard_worker` выполняет native clipboard worker, timeout и recovery.
Настройки пишет backend (`SettingsWriter` в демоне); UI получает snapshot и отправляет
`SetSettings`.

Системные эффекты, которые нельзя держать в UI, живут в `infra/`: `dialogs` (выбор файла,
папки и каталога), `desktop` (reveal), `files` (создание каталога), `log_file` (чтение лога
программы), `clipboard` (синхронное копирование текста), `audio` (выход звука), `native_window`
(анимация открытия, исключение из захвата). UI вызывает эти функции и не содержит `rfd`,
`std::fs`, `std::thread` и платформенных модулей эффектов — это проверяет тест границ.

### 7.5. Shared app mechanisms

- `features/intent.rs` — `Intent`: единственный вход из UI в логику приложения.
- `features/window_presence.rs` — `WindowPresence`: скрытость окна, ghost-режим и quiet-режим.
- `features/recording/plan.rs`, `features/screenshot.rs` — чистые решения записи и снимка.
- `features/jobs.rs` — one-shot `Job<T>` с owned handle и однократным poll.
- `features/latest_worker.rs` — bounded replacement worker для устаревающих запросов.
- `features/notices.rs` + `ui/notices.rs` + `app/notices.rs` — уведомления. Логика: очередь,
  уровни, срок жизни, hover pause, silent actions. Вид: `ui/notices.rs` рисует карточки и анимацию
  появления/закрытия. `app/notices.rs` — только `App::error/warning/info/success`, которые
  формируют текст и пушат уведомление.
- `infra/tray.rs` — иконка трея. Горячие клавиши разделены: `infra/hotkeys.rs` держит
  `GlobalHotKeyManager`, регистрацию и событие нажатия (`Wake`), `features/hotkeys.rs` — слоты,
  конфликты и группировку комбинаций, `ui/pages/settings/hotkeys.rs` — захват нажатия в строку
  комбинации (`combo_from_key`, `combo_from_token`).
- `wake.rs` — `Wake`: просьба о перерисовке вместо `egui::Context`; его принимают `features` и
  `infra`, а `app` превращает в `ctx.request_repaint*`.
- `test_support.rs` (корень, только в тестовой сборке) — общий мьютекс harness-тестов и локальные
  guard'ы (`LocaleGuard`, `english_locale`, `russian_locale`). Не принадлежит слоям: им пользуются
  тесты и `ui`, и `app`, поэтому он не должен жить в `app`.
- `ui/themes/`, `ui/widgets/`, `ui/caches.rs`, `icon.rs`, `i18n.rs` — presentation and
  localization. `ui/caches.rs` держит текстуры представления (логотип по теме). `icon.rs` и
  `i18n.rs` остаются в корне крейта: их используют и `features` (тексты уведомлений), и `infra`
  (иконка трея), а `features` не может зависеть от `ui`.
- Region editor разделён на два слоя. `features/region_editor.rs` хранит модель и решения: `Purpose`,
  `Outcome`, `SaveDefaults`, выбор формата по расширению (`resolve_save_path`), порядок форматов,
  имя файла и запись кропа (`save_crop`: crop, аннотации, сохранение). `ui/region_editor.rs` —
  отдельный native viewport: текстура кадра, пересчёт координат, ввод, отрисовка, текст и панели;
  `ui/region_editor/editor.rs` — редактирование текста. Selection определяет итоговый crop, но
  annotations хранятся в координатах полного кадра и видны во всём editor viewport. Активный text
  editor имеет рамку, перетаскивается за её поля и фиксируется кликом снаружи. Overlay использует
  один native viewport; opaque viewport создаётся скрытым заранее и переиспользуется при открытии;
  `Tab` меняет monitor frame через один capture job без создания нового окна. Сохранение области
  идёт через системный диалог (`infra/dialogs`), уровень окна на время диалога меняет viewport;
  формат берётся из расширения, а overlay закрывается только после успешной записи. Отключение
  анимации открытия и исключение из захвата — `infra/native_window`. Перед снимком области окно
  скрывается и выдерживается `HIDE_DELAY`, затем показывается overlay.

### 7.6. Reusable widgets and themes

Переиспользуемые UI-примитивы живут только в `crates/app/src/ui/widgets/`, по одному
виджету на файл:

```text
crates/app/src/ui/widgets/
  mod.rs
  button.rs
  icon_button.rs
  layout.rs
  menu_item.rs
  modal.rs
  segmented.rs
  track_slider.rs
```

`ui/widgets/mod.rs` объявляет модули и реэкспортирует публичные функции. Страницы и остатки
`features` обязаны использовать эти примитивы вместо локальных копий кнопок, иконок, модалок,
сегментов и слайдеров. В `ui/pages/` и `features/` остаётся только композиция конкретного экрана и
его бизнес-состояние; новый общий виджет добавляется отдельным файлом в `ui/widgets/`.

Темы и их палитры живут в `crates/app/src/ui/themes/`:

```text
crates/app/src/ui/themes/
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

Запись выполняет `infra/settings_file`: изменения объединяются, последняя версия
сохраняется вне UI, файл публикуется атомарно. Каталоги данных задаёт
`core::paths::data_dir()` через `ProjectDirs::data_local_dir()` (fallback —
`std::env::temp_dir()/MediaA`): `logs/` — логи, `work/` — рабочие файлы незавершённых операций. Медиатека по
умолчанию находится в `Documents/MediaA`, а не в каталоге настроек.

### 7.8. Слои `app` и правило направления

Правило согласовано 01.11.2026. Оно действует для нового кода сразу; существующие нарушения
исправляются этапами `ARCH-14.2`…`ARCH-14.6`
([tasks/ui-logic-separation.md](tasks/ui-logic-separation.md)).

- `features/` — логика приложения: владеет состоянием, фоновой работой и terminal result. Без
  `egui`/`eframe` и без обращения к `ui` и `app`. Перерисовку просит через `wake::Wake`.
- `ui/` — отрисовка и ввод. Получает read-only срезы, изменения запрашивает через `Intent`. Не
  вызывает ОС, файлы, потоки и системные диалоги — для них есть `infra/`.
- `infra/` — эффекты ОС. Не знает ни про `ui`, ни про `features`; ждёт `wake::Wake`, а не
  `egui::Context`.
- `app.rs` — композиция: единственное место с `egui::Context` и мутирующим доступом к `features`.

Правило действует и на тестовый код: гейт сканирует `#[cfg(test)]`-блоки и файлы `tests.rs` для
`features` и `infra` без вырезания, поэтому тест логики не может импортировать `ui`, а тест
адаптера — `ui` или `features`. Для `ui` тестовый код исключён: ему нужны `egui` и harness. UI-регресс
тесты и снапшоты живут рядом с отрисовкой в `ui/`, даже если проверяют поведение контроллера через
экран. Тесты, которые форматируют локализованный текст, берут общий мьютекс и фиксируют язык
(`english_locale`/`russian_locale`): иначе глобальная локаль может смениться во время теста. Тесты
статуса конвертации замораживают `elapsed` через тестовый хук — время не должно влиять на снапшот.
Исключений для `ui/pages/agent` больше нет: UI-страницы агента работают только с protocol
(`AgentSnapshotWire`, события и команды), а `mediaa-agent/engine/queue` в frontend не входят.

Проверка границ — одна команда: `cargo test -p mediaa-frontend --lib modules_respect_architecture_boundaries`:
`features` не видит `ui`/`app`/`egui`, `ui` не зовёт ОС и файлы, `infra` не видит `ui`/`features`/`egui`; `ui` не импортирует `crate::app` (страницы работают через `Ctx`).
Правила и список временных исключений живут в `crates/frontend/src/boundaries.rs`; каждое исключение
называет этап, который его удаляет.

### 7.9. Границы между крейтами

Направление зависимостей между крейтами фиксирует отдельный dev-крейт `crates/arch`
(`mediaa-arch`, в тестовой сборке): allowlist обычных и dev-зависимостей, полный охват
workspace, запрет зависимости от приложения и внутренних build-зависимостей, а также
feature-гигиена: `testing` только в dev-зависимостях, `mediaa_engine::testing` только в тестах.

```text
cargo test -p mediaa-arch
```

Правила — `mediaa_arch::RULES`, подробности и порядок изменений — `specs/tasks/crate-boundaries.md`.
`cargo` по-прежнему ловит отсутствующие зависимости и циклы; тест закрывает то, что компилятор не
видит: «зависимость объявлена, но не имеет права быть».

### 7.10. Backend: структура крейта

```text
crates/backend/src
  daemon/            composition root и event loop
    mod.rs           Daemon, run, tick
    dispatch.rs      IPC/event dispatch
    agent.rs         агентские команды
    services.rs     library/preview/conversion/recording/recovery
    notices.rs       NoticeRing
    launch.rs        launcher/daemon/ui процессы
  features/          предметные сервисы
    agent/           host, snapshot, requests
    library/         LibraryStore
    preview/         service + store
    capture/         capture file/commit
    clipboard/       clipboard service
    recording/       controller, host, plan
    conversion/      controller
    hotkeys.rs       разбор комбинаций и конфликты
  infra/             ipc, tray, hotkeys, settings_file
  runtime.rs         tokio runtime
```

`daemon` — единственный composition root: только он знает все feature-сервисы и
`infra`. `features` не должны зависеть от `daemon`; `infra` не знает про
`features`. Backend не содержит UI-слоя: у него нет `ui`/`app` frontend-крейта.

Проверка backend-границ: `cargo test -p mediaa-backend --lib boundaries`.

### 7.11. Роли демон и UI

Приложение запускается в одной из трёх ролей: лаунчер (`mediaa.exe` без флага), демон (`--backend`)
и UI (`--ui`); `--daemon` — старый алиас демона. Лаунчер поднимает демон, если его нет, и запускает UI; повторный запуск при
открытом UI не создаёт второй экземпляр и второй демон. Демон владеет single-instance и слушает
локальный канал: `infra/ipc` поверх `interprocess`, имя — из `platform::ipc` (named pipe в
Windows, Unix-сокет с правами 0600). Сообщения — JSON-строки с версией протокола в handshake;
несовпадение версии и закрытие канала обрабатываются как ошибка. UI подключается к демону и
завершается, когда канал закрылся; UI не перезапускает демон и не берёт его single-instance.

Сейчас демон владеет треем, глобальными хоткеями, жизненным циклом, файлом настроек
(`SettingsWriter` в демоне; UI получает `Snapshot`/`Changed` и присылает `SetSettings`),
recovery, записью, индексом библиотеки, capture, previews, conversion и агентом. UI владеет
окном, страницами, transport и чтением файлов.
Повторный запуск при уже открытом UI не создаёт второй процесс: лаунчер видит
`metadata.ui` в `Welcome` и просит демон отправить существующему UI `show_window`
(окно снимается с minimized/скрытия и фокусируется). Закрытие окна при `close_to_tray = true` завершает UI-процесс, демон остаётся в трее; работа
(запись, конвертация, capture, recovery, оверлей) продолжается, а окно вместо выхода скрывается
и UI завершается через 3 с после её окончания. При `false` UI шлёт команду `quit`, и демон выходит.
Демон поднимает UI по требованию: лаунчер — видимым, фоновая команда трея или хоткея — скрытым
(`--tray`). Запись: демон держит `RecordingController`, гоняет `host` и присылает
store `recording`; UI шлёт `recording_*`-команды, рисует `Snapshot`/`Changed` и применяет
recording-события (окно, файлы, завершение с clipboard/звуком). На время холодного старта пикера демон показывает
своё layered-окно-маску и скрывает его по команде `picker_shown` от UI. Автозапуск в режиме трея
запускает `--backend`. Логи раздельные по роли, проблемы старта демона приходят в UI через
эфемерный ring уведомлений (`Notices` при подключении, `Notice` для новых). Recovery
незавершённых записей выполняет демон при старте и по команде recovery от UI; UI получает
store `recovery { running }` и событие `RecoveryReport`, затем показывает результат уведомлением. Демон владеет индексом библиотеки: загрузка, скан, `refresh_paths` и `flush`; watcher
публикует store `library`, а UI перезагружает индекс только для чтения и рисует миниатюры.

Capture: UI скрывает окно и просит backend снять кадр; backend отдаёт `CaptureSourceWire` с
backend-issued путём, UI компонует финальный кадр и отправляет `screenshot_commit` (файл и
clipboard делает backend). UI не вызывает capture/render/save backend-функции напрямую.

Агент: `AgentHost` всегда живёт в backend-процессе и тикает независимо от UI. UI читает
snapshot/live-события и отправляет команды; при подключении UI backend ничего не роняет и
второй хост не создаёт.
([tasks/background-first.md](tasks/background-first.md)).

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

### 7.11. Единая синхронизация

Весь доменный обмен идёт через `mediaa-sync` и `mediaa-protocol`; отдельного набора
`Message`-вариантов больше нет. Транспорт — `interprocess` named pipe, кадры с `u32`
длиной и JSON-кодеком за трейтом `Codec`.

Конверт:

- `Hello { version, client }` — `ui` или `launcher`; `Welcome { version, session, event_seq,
  metadata }` (`metadata.tray`) или `Reject`;
- `Subscribe { stores, versions, event_seq }` → `Snapshot`/`Changed`/`Event`;
- `Command { id, name, payload }` → `Result { id, .. }`; сервер кэширует результат по id;
- `Event { seq, kind, payload }`, `Ping`/`Pong`, `Resync`.

Состояние — `StateHub` в демоне: версия на store, коалесинг до `flush`, клиент применяет
только более новую версию. События — `EventLog` с seq; новый клиент получает `event_seq`
в `Welcome` и не принимает первый живой event за gap. Потеря `Changed`/события лечится
`Resync`; переподключение присылает актуальные `Snapshot`. Демон шлёт `Ping` каждые
5 с и после 15 с без `Pong` считает UI зависшим и отключает его.

Store: `settings`, `recording`, `library`, `notices`, `recovery`. События: recording,
recovery, tray, hotkeys. Команды: `set_settings`, recording-команды, `hotkeys_paused`,
`tray_state`, `quit`, `picker_*`.

UI буферизует кадры, пришедшие до создания `App`, и проигрывает их через тот же `Client`.
`pending_events` в демоне вливаются в EventLog сразу после `Welcome`, поэтому трей-команда,
пришедшая до UI, не теряется.

Замеры 02.11.26 (debug, Windows 11): демон idle 18.3 MB / 0.3 % одного ядра; UI settings
на GL — 328.5 MB / 0 %; UI settings на Vulkan — 591.4 MB / 99 % (известное поведение
eframe, не sync). Смоук записи: `Finished` доходит, файл попадает в clipboard; `--area`
открывает пикер. Видео-энкодер по умолчанию использует `-tune zerolatency`
(`RecordSettings.encoder_tuning`): это резко снижает память libx264; потоки/lookahead
можно включить параметрами.