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

Крейт зависит от `core`. Он отвечает за native API и платформенные различия.

### 2.1. `screen`, `stream`, `wgc`, `x11`

Перечисляют мониторы и окна, создают источники кадров и реализуют платформенный capture.
Windows Graphics Capture, X11/MIT-SHM и Wayland/PipeWire отличаются внутри этого слоя.

### 2.2. `audio`

Предоставляет микрофоны, system audio, `CpalSource` и confirmation tone.

### 2.3. `clipboard`, `files`

Выполняют native clipboard, open/reveal и перемещение файлов в trash.

### 2.4. `window`, `outline`, `region_marker`, `cursor`, `cursor_overlay`

Управляют native window visibility, подсветкой области и отрисовкой курсора.

### 2.5. `single_instance`, `startup`, `keys`

Обеспечивают single-instance scope, start mode и OS-level keyboard polling.

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

### 3.2. `animation_backends`, `webp_decode`

Кодируют анимацию одним полным проходом Direct: WebP через FFmpeg/libwebp, GIF через
FFmpeg. Фильтры и WebP-части приходят из `mediaa-webp`/`mediaa-gif`.
`WebpMode::{Quality, Fast}` задаёт method и ограничения fps/высоты. Сегментного и
live-WebP больше нет.

### 3.3. `convert`, `filters`

Строят FFmpeg-фильтры и выполняют пользовательские конвертации.

### 3.4. `info`, `preview`, `preview_cache`, `thumbnail`

Определяют медиа metadata, hover-preview, proxy и thumbnail operations.

### 3.5. `error`

`error` классифицирует ошибки, от которых зависит пользовательское действие или recovery.
FFmpeg execution/cancel живёт в отдельном `mediaa-ffmpeg`.

## 4. Форматные крейты

- `mediaa-ffmpeg` — запуск/отмена FFmpeg, общий слой для форматов.
- `mediaa-webp` — WebP-граф, normalize/validate/trim, merge animated WebP.
- `mediaa-gif` — GIF-граф, палитра и dither.

`mediaa-media` не содержит сегментного pipeline; форматные операции вызываются через
эти крейты.

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
- `theme.rs`, `icon.rs`, `i18n.rs` — presentation and localization.
- `overlay.rs` — screenshot editor. Selection определяет итоговый crop, но annotations хранятся в
  координатах полного кадра и видны во всём editor viewport. Активный text editor имеет рамку,
  перетаскивается за её поля и фиксируется кликом снаружи. Overlay использует один native viewport;
  opaque viewport создаётся скрытым заранее и переиспользуется при открытии; `Tab` меняет monitor
  frame через один capture job без создания нового окна. Сохранение области идёт через системный
  диалог; формат берётся из расширения, а overlay закрывается только после успешной записи.
  Перед снимком области окно скрывается и выдерживается `HIDE_DELAY`, затем показывается overlay.

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
  <- mediaa-webp
  <- mediaa-gif
  <- media <- mediaa-ffmpeg, mediaa-webp, mediaa-gif
  <- engine
  <- queue
  <- agent <- engine, queue
  <- eval <- agent, engine, queue
  <- app <- все перечисленные, кроме eval
```

`core` — общий фундамент, а не строго чистая domain model. Новые OS API и FFmpeg зависимости в
него не добавляются.
