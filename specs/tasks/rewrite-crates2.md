# Переписывание MediaA в `crates2`

Статус: предложение. Новый код ещё не создан; старый проект остаётся эталоном поведения
и источником медиа-алгоритмов.

## 1. Решение коротко

- Новый отдельный workspace `crates2/` внутри репозитория. Старый workspace не меняется до
  паритета.
- Девять крейтов вместо девятнадцати. У каждого одна причина существования.
- Один `mediaa.exe`, три роли: лаунчер, `--backend` (трей, живёт всегда), `--ui` (окно по
  требованию).
- Backend — единственный владелец доменного состояния, файлов, БД, записи, конвертации и
  снимков. Frontend — проекция состояния, ввод и отрисовка.
- Backend на Tokio. Блокирующие операции (FFmpeg, SQLite, скан папки) — через `spawn_blocking`.
- Один типизированный протокол. Без `StateHub`, `EventLog`, `Subscribe`, `Resync`, `Ping`,
  версий store и строковых имён команд.
- Один путь capture: backend снимает кадр, UI выбирает область и присылает вектор аннотаций.
- Агент в `crates2` не входит.
- `i18n` — отдельный крейт. SQL — отдельный крейт `storage`.
- Задачи ST-2…ST-12 в старом дереве не входят в этот план: их цель (одно состояние, чистые
  переходы, типизированный протокол) реализуется здесь сразу.
- Цель: около 26–28 тысяч строк production-кода против примерно 50 тысяч неагентских строк
  сейчас; агентские ~35 тысяч строк не переносятся.

## 2. Что есть сейчас

### 2.1. Продукт

MediaA — программа для Windows: скриншоты, запись экрана/окна/области, запись звука,
конвертация медиа и библиотека результатов.

Экраны и поведение, которые нужно сохранить:

| Экран | Что есть |
|---|---|
| Оболочка | окно 1120×760 (минимум 480×480), шапка 46 px, разделы Library / Capture / Convert, Settings и About, подвкладки Capture: Screenshot / Screen / Audio |
| Library | сетка карточек, поиск, фильтр по типу, сортировка, выделение, массовые действия, hover-превью, просмотрщик с зумом/звуком/таймлайном, rename, delete в корзину, открытие и показ в папке |
| Screenshot | снимок области, полного экрана и окна; после снимка — PNG и/или копирование; окно выбора области с аннотациями |
| Record | цель (область / экран / окна), список мониторов и окон, профиль записи, микрофон и системный звук, старт/пауза/стоп/отмена, статус |
| Record audio | микрофон и/или системный звук, MP3 / M4A / WAV / FLAC / OGG, битрейт, старт/стоп |
| Convert | drag&drop и выбор файлов, матрица форматов, качество/разрешение/битрейт, папка и имя, очередь, прогресс, отмена, ошибки по файлам |
| Settings | appearance, startup, previews, window/tray, folders, hotkeys, профили записи, простой/расширенный режим, сброс |
| About | версия, ссылки, логи |
| Overlay | замороженный кадр монитора, выделение, ручка/линия/стрелка/прямоугольник/маркер/текст, цвета, толщина, undo, размер, Tab между мониторами, копировать/сохранить/отмена |
| Уведомления | карточки с уровнями, TTL, раскрытием, действиями, анимацией; модалки блокируют ввод |
| Трей и хоткеи | меню трея, прогресс записи, глобальные хоткеи, автозапуск, single instance |

Детали, без которых «тот же UI» не получится:

- **Оболочка.** Окно 1120×760, минимум 480×480, прозрачный фон. Шапка 46 px: лого 24,
  «MediaA», секции Library / Capture / Convert, справа Settings / About; при ширине < 820
  секции превращаются в ComboBox. Подвкладки Capture — 36 px: Screenshot / Screen / Audio.
  Контент — margin 18×14, ScrollArea с запоминанием offset по странице. Красная точка на
  Capture при записи, зелёная на Convert при конвертации.
- **Screenshot.** Две плитки: Select area и Full screen (с хоткеями), карточка Displays
  (refresh, строки `📷 name (W×H)` + `primary`), карточка Windows (refresh, `📷 title — app`,
  пустая подсказка). Во время другой работы — предупреждение.
- **Screen (Record).** Три плитки Region / Screen / Windows; карточка источников 150 px;
  профиль: Format (segmented), Preset, Resolution (комбо + Custom 16..4320), FPS 5..60,
  Quality 1..100, cursor, no-signal, allow-unavailable, hide-to-tray; справа Audio
  (sound, mic Auto/Off/device, system) или Animation (colors 4..256, dither, WebP quality).
  Start 220×36; при записи контент disabled.
- **Статус записи.** Плавающая карточка снизу: Starting (таймер, Preparing, Cancel→модалка);
  Recording (точка, таймер 28, источники/кадры/звук, Cancel, Stop and save, pause);
  Saving (recorded, прогресс, Abort→модалка). При `hide_recording_preparation` карточка
  Starting скрыта.
- **Audio.** Output (Format, Preset для lossy, bitrate 96/128/192/256/320) и Sources
  (mic, system sound с подсказкой ОС); Start 220×36.
- **Convert.** Drag&drop, таблица файлов (Name/Type/Size/Duration/Status/Actions), выбор
  файлов, удалить все, retry; матрица форматов по группам Video / Anim / Image / Audio;
  quality, effort (Fastest/Fast/Balanced/Max), resolution, FPS, remove sound + kbps + mono,
  параметры анимации и still, output folder; Start 240×38; плавающий статус с Cancel.
- **Library.** Сетка 200 px, карточки с расширением/размером/W×H/длительностью и звуком;
  hover 320 px / 15 fps / 240 кадров; сортировка, фильтр, поиск, refresh, open folder,
  cleanup week/month; выделение Ctrl/Shift; bulk-бар Copy files/paths, Convert, Rename,
  Clear, Delete; меню карточки open/reveal/copy file/image/path/link/convert/rename/
  select/delete; viewer с play/pause, временем, громкостью, mute, скоростью 0.5–2,
  таймлайном, зумом 0.1–10 и панорамированием; диалоги delete/cleanup/rename.
- **Settings.** Reset, Advanced; Appearance (Theme System/Light/Dark/Lime/Peach, Language
  System/en/ru, notifications), Startup (autostart, start in tray), Previews (animated,
  full videos), Window (tray left click, stop on left click, tray progress, close to tray,
  highlight region + width 2..32), Folders (media, convert), Hotkeys (enable + 5 полей),
  профили Hotkey / Region / Audio. Simple-режим показывает только часть полей; advanced-off
  сбрасывает скрытые значения (в новом коде это поведение сохраняем, но нормализацию при
  загрузке делаем одинаковой).
- **Region editor.** Отдельное окно без декораций поверх монитора; инструменты select/pen/
  line/arrow/rect/marker/text, 8 цветов, толщина 2/4/8/12, undo; панель Copy / Save… /
  Record area / Cancel; Enter, Ctrl+C, Ctrl+S, Ctrl+Z, Ctrl+A, стрелки, Shift+стрелки, Tab,
  Esc; бейдж `W × H`; сохранение PNG/WebP через системный диалог; для выбора области записи
  панель инструментов скрыта.
- **Темы и виджеты.** Четыре палитры + System; радиусы 4/6/7/8/10/12, отступы 6×6, шрифты
  Heading 18 / Body 13 / Button 13 / Small 11 / Mono 12, Phosphor-иконки, логотип по теме.
- **Уведомления.** Info/Success/Warning/Error, 5/8/10 с (important 30), максимум 5, hover
  пауза, разворот, Copy/Reveal, лог-ID, блокировка при модалке.
- **i18n.** 865 ключей в en/ru; после удаления агента остаётся ~410. Все строки — в
  `mediaa2-i18n`.

### 2.2. Процессы

Сейчас один exe запускается в одной из ролей: лаунчер, `--backend` (демон) и `--ui`.
Демон владеет треем, хоткеями, настройками, записью, индексом библиотеки, capture, previews,
конвертацией и агентом. UI владеет окном, страницами и частью эффектов. Закрытие окна при
`close_to_tray = true` завершает UI-процесс; скрытый UI, поднятый для фоновой команды,
завершается через 3 секунды.

### 2.3. Размер

Строки Rust по крейтам (включая тесты, по состоянию рабочего дерева):

| Крейт | Строк | Судьба |
|---|---:|---|
| frontend | 39 393 | переписать; агентский UI выбросить |
| agent | 22 726 | не переносить |
| core | 9 599 | сократить, агентские модули выбросить |
| backend | 7 843 | переписать |
| queue | 7 685 | не переносить |
| media | 6 683 | перенести алгоритмы |
| platform | 4 523 | перенести Windows-часть |
| eval | 2 652 | не переносить |
| engine | 2 498 | не переносить |
| storage | 2 003 | переписать |
| protocol | 1 387 | переписать |
| sync | 1 169 | удалить |
| webp | 809 | влить в media |
| render | 738 | влить в media |
| ffmpeg | 564 | влить в media |
| app | 388 | переписать |
| gif | 247 | влить в media |
| app-agent | 28 | не переносить |

Без агентских крейтов (`agent`, `queue`, `engine`, `eval`, `app-agent`) остаётся около
76 тысяч строк. Из них агентский UI, настройки и модели — ещё примерно 15 тысяч. Реальная
неагентская база — около 50 тысяч строк production.

### 2.4. Почему переписываем, а не достраиваем

1. **Агент пророс во всё.** `Settings` содержит `agent`, core содержит чаты, каталоги,
   транскрипты, слоты, поиск; frontend содержит 10+ агентских экранов. Даже если агент не
   нужен, его типы и зависимости остаются в сборке.
2. **Разделение на процессы не закончено.** `backend-split-plan` фиксирует незакрытые F8,
   F9, C1–C3. UI всё ещё зависит от `mediaa-media`, `mediaa-ffmpeg`, `global-hotkey` и
   держит контроллеры, которые должны быть в backend.
3. **Два владельца одного состояния.** `RecordingController` + `RecordingView` +
   `ViewState` + `RecordingViewWire`; `AppCapture` + `CaptureClient` + backend `capture`;
   настройки в UI и в backend; `WindowPresence` + `WindowLifecycle` + `TrayMachine`.
4. **God objects.** `backend::daemon::Daemon` — 725 строк и 44 поля;
   `frontend::app::App` — 872 строки и около 60 полей; `Ctx` — 26 полей;
   `Intent` — десятки вариантов; `ConversionController` — 707 строк с UI-флагами.
5. **Строковый протокол.** Около 60 констант-имён, отдельные `encode_*`/`decode_*` на каждый
   тип, `Frame::Command { name: String, payload: Vec<u8> }`, `StateHub` с версиями,
   `EventLog` с seq, `Resync`, `Ping/Pong`. Типизация начинается только в ST-9, которая ещё
   не сделана.
6. **Три пути capture.** UI снимает кадр через WGC; backend снимает кадр и пишет RGBA-файл;
   UI читает файл, компонует и пишет обратно; backend читает и сохраняет. На каждый путь —
   свои состояния, клиенты и ошибки.
7. **Жизненный цикл лечится флагами.** `show_ack_pending`, `show_retry_at`, `spawned_at`,
   `quit_when_idle`, `idle_since`, `tray_applied`, `picker_shown_sent`. Трей-иконка
   управляется **из UI** командой `tray_state`, хотя записью владеет backend; автозапуск
   настраивает UI, а не backend. Пользователь сообщает, что трей работает не полностью;
   это ожидаемо при таком наборе флагов.
8. **Несколько рантаймов и ранний `exit(0)`.** Демон работает на std-потоках и
   `crossbeam`, но внутри создаёт два Tokio-рантайма (preview и агент). При выходе
   `process::exit(0)` убивает финализатор записи; файл остаётся только в recovery.
9. **Протокол теряет события при переподключении.** На каждый коннект `Server::new()`
   обнуляет hub и EventLog; `Finished`/уведомление, отправленные в момент смерти UI,
   теряются. `pending_events` спасает только случай «UI вообще не было».
10. **Индексация зависит от UI.** Скриншот коммитит backend, но `library_add` посылает UI;
    выход конвертации не индексируется вообще.
11. **Тесты есть, но состояние не изолировано.** 211 UI-тестов и 76 снапшотов проверяют
    экраны, но доменные переходы размазаны между процессами и флагами; тестов на
    `Daemon`/`dispatch` нет. Инициатива ST (12 задач) как раз пытается это исправить;
    переписывание делает то же самое сразу и без наследия.

Вывод: достройка требует тех же изменений, но с постоянной совместимостью со старыми
контрактами. Чистая модель меньше суммы текущих адаптеров.

Что чиним попутно, не меняя UX:

| Симптом | Причина сейчас | Решение |
|---|---|---|
| первый клик трея поднимает UI, который сразу выходит | `TrayMachine` + ack-таймауты + резидентный UI | один `Presence`, UI живёт до закрытия окна |
| события теряются после переподключения UI | `Server::new()` на каждый коннект | `State` при подключении, без event log |
| выход убивает финализатор записи | `process::exit(0)` | остановить захват, оставить work dir, recovery при старте |
| выход конвертации не в библиотеке | индексация только в UI/record | сервис добавляет результат после публикации |
| трей показывает состояние из UI | команда `tray_state` | tray view выводится из состояния backend |
| автозапуск настраивает UI | `startup::configure` во frontend | backend настраивает autostart |
| два писателя SQLite | `LibraryStore` + `PreviewStore` | одно соединение в library-сервисе, превью без БД |
| два Tokio-рантайма | preview + agent host | один runtime backend |

## 3. Целевая архитектура

### 3.1. Принципы

1. **Один владелец.** Доменное состояние влияет на файлы, процессы, запись или других
   клиентов — владелец backend. Только клиентский опыт (страница, скролл, выделение, модалка,
   черновик настроек, текстуры) — владелец frontend.
2. **Backend авторитетен, frontend проецирует.** UI не придумывает `Recording`/`Saving`, не
   пишет БД, не запускает FFmpeg и не сохраняет снимки.
3. **Состояние — enum, переход — событие, результат — эффект.** Никаких `bool`-фаз. Время
   передаётся в переход, а не читается внутри.
4. **Один контракт.** Типы домена сериализуются напрямую; отдельного `*Wire`-слоя нет.
5. **Один capture.** Backend снимает; UI только выбирает и рисует аннотации.
6. **UI — гость.** Окно можно закрыть в любой момент; backend продолжает запись, конвертацию
   и библиотеку. Резидентного скрытого UI нет.
7. **Меньше кода.** Нет compatibility-слоёв, миграций БД, DI, event bus, plugin-архитектуры,
   второго event loop и «временных» адаптеров.

### 3.2. Роли и жизненный цикл

```text
mediaa.exe                  лаунчер: поднять backend, попросить окно, выйти
mediaa.exe --backend        трей-демон: состояние, файлы, БД, запись, снимки, конвертация
mediaa.exe --ui             окно; --show / --tray / --picker <id>
mediaa.exe --ui --area      окно выбора области (отдельный процесс)
```

| Роль | Живёт | Владеет |
|---|---|---|
| launcher | секунды | single instance, запуск backend/UI |
| backend | до «Выхода» | tray, hotkeys, settings, library, previews, recording, conversion, screenshots, notices, жизненный цикл UI |
| ui | до закрытия окна | окно, страницы, overlay, viewer, диалоги, текстуры, черновик настроек |

Таблица жизненного цикла:

| Событие | Backend | UI |
|---|---|---|
| старт с автозапуском | `--backend`, окна нет | не запущен |
| клик «Открыть окно» | нет UI → `Spawn(Show)`; есть → `ShowWindow` | показывает окно |
| повторный запуск exe | лаунчер видит backend и шлёт `ShowWindow` | существующий UI показывает окно |
| закрытие окна, `close_to_tray = true` | UI отключился, работа продолжается | процесс завершается |
| закрытие окна, `close_to_tray = false` | получает `Quit`, завершает задачи | процесс завершается |
| «Выход» из трея | `Quit` в UI, ждёт отключения ≤ 2 с, завершает задачи | выходит |
| падение UI | сбрасывает presence, отменяет picker | — |
| падение backend | — | ошибка, диалог, выход |

Следствия:

- нет `--tray`-режима с трёхсекундным выходом;
- нет `quit_when_idle`, `show_ack_pending`, `spawned_at`;
- повторный клик «Открыть» не создаёт второй UI: presence либо шлёт `ShowWindow`, либо
  находится в `Starting` и второй процесс не запускает;
- запись из трея не требует UI;
- регион-пикер — отдельный процесс, ему не нужен резидентный UI.

Лаунчер:

1. Подключиться к backend; если нет — запустить `--backend`, ждать ≤ 5 с.
2. `--area` / `--fullscreen` → отправить `Picker(Start)` и выйти (пикер поднимет backend).
3. `--page=...` → запустить `--ui --show --page=...`.
4. Иначе, если UI подключён — `ShowWindow`; если нет — запустить `--ui --show`.
5. Выйти. Лаунчер не держит состояние и не является вторым владельцем.

### 3.3. Крейты

| Крейт | Ответственность | Зависит от | Бюджет, строк |
|---|---|---|---:|
| `mediaa2-core` | настройки, форматы, геометрия, записи библиотеки, пути, имена, атомарная запись, `Shape`, `Frame` | внешние | 3 800 |
| `mediaa2-i18n` | языки, locale-файлы, `t!`, проверка ключей | `rust-i18n` | 150 + 2 TOML |
| `mediaa2-protocol` | сообщения, состояние домена, `State`, страницы библиотеки | core, serde_json, tokio-io | 700 |
| `mediaa2-storage` | единственное место SQL: схема, миграция-пересоздание, CRUD записей | core, rusqlite | 700 |
| `mediaa2-platform` | Windows: screen/WGC, окна, cursor, audio, clipboard, tray, hotkeys, single instance, autostart, IPC endpoint, process spawn | core, tokio, interprocess | 3 400 |
| `mediaa2-media` | recorder, convert, GIF, WebP, FFmpeg-jobs, preview, screenshot compose, аннотации | core, platform | 6 000 |
| `mediaa2-backend` | Tokio-демон, сервисы, чистые переходы, IPC-сервер | core, i18n, protocol, storage, media, platform | 3 200 |
| `mediaa2-frontend` | egui-приложение, страницы, overlay, viewer, клиент, проекция | core, i18n, protocol, platform, media (только чтение) | 9 000 |
| `mediaa2` (bin) | роли, лаунчер | backend, frontend | 150 |

`media` включает `ffmpeg`, `gif`, `webp` и `render` модулями. Отдельные форматные крейты не
нужны: у них нет независимых потребителей, а границы уже держатся модулями.

`frontend` зависит от `media` только ради чтения: кадры viewer и звук. Это осознанное
исключение; запись, конвертация и сохранение остаются в backend.

### 3.4. Направление зависимостей

```text
core
 ├── protocol ──────────────┐
 ├── storage ───────────┐   │
 ├── platform ───────┐  │   │
 └── media ──────┐   │  │   │
                 │   │  │   │
        backend ─┴───┴──┴───┘
        frontend ── protocol, core, i18n, platform, media(чтение)
             │
            app

i18n (только rust-i18n) ── backend, frontend
```

Запрещено:

- `frontend` → `storage`, `backend`;
- `storage` → что-либо, кроме `core`;
- `protocol` → `platform`, `media`, `backend`, `frontend`;
- `backend` → `frontend`;
- `i18n` → что-либо, кроме `rust-i18n`.

### 3.5. Раскладка

```text
crates2/
  Cargo.toml
  AGENTS.md                 правила нового проекта
  README.md
  crates/
    core/src/
      lib.rs
      settings.rs
      formats.rs
      geometry.rs
      library.rs
      paths.rs
      naming.rs
      atomic_file.rs
      annotate.rs
      frame.rs
    i18n/
      src/lib.rs
      locales/en.toml
      locales/ru.toml
    protocol/src/
      lib.rs
      message.rs
      state.rs
      library.rs
      wire.rs
    storage/src/
      lib.rs
      schema.rs
      entries.rs
    platform/src/
      lib.rs
      screen.rs
      window.rs
      audio.rs
      clipboard.rs
      tray.rs
      hotkeys.rs
      instance.rs
      startup.rs
      ipc.rs
      process.rs
      files.rs
      dialogs.rs
    media/src/
      lib.rs
      recorder/
      convert.rs
      ffmpeg/
      gif/
      webp/
      preview.rs
      screenshot.rs
      annotate.rs
    backend/src/
      lib.rs
      run.rs
      state.rs
      ipc.rs
      services/
        settings.rs
        library.rs
        previews.rs
        recording.rs
        conversion.rs
        screenshots.rs
        presence.rs
        notices.rs
      domain/
        recording.rs
        conversion.rs
        picker.rs
        presence.rs
        settings.rs
    frontend/src/
      lib.rs
      app.rs
      client.rs
      view.rs
      ui_state.rs
      pages/
        library.rs
        record.rs
        audio.rs
        convert.rs
        settings.rs
        about.rs
        capture.rs
      picker.rs
      viewer.rs
      notices.rs
      widgets/
      theme/
      textures.rs
    app/src/
      main.rs
      launcher.rs
```

### 3.6. Сборка и проверки

- `crates2` — отдельный workspace; старый `Cargo.toml` не меняется.
- FFmpeg/vcpkg-окружение то же: `scripts/windows/build/env.ps1`, затем
  `cargo --manifest-path crates2/Cargo.toml build`.
- Быстрая проверка: `cargo --manifest-path crates2/Cargo.toml check --workspace --lib --bins`.
- Тесты: `cargo --manifest-path crates2/Cargo.toml test --workspace`; media-тесты — отдельно.
- Новые скрипты `crates2/scripts` только dot-source-ят корневой `env.ps1`, без копирования
  vcpkg-логики.

## 4. Протокол и синхронизация

### 4.1. Транспорт и правила

- Локальный duplex: named pipe `\\.\pipe\mediaa2-{scope}` на Windows, Unix-сокет с правами
  0600 на остальных.
- Один канал на процесс. Отдельного `CLIENT_CAPTURE` нет.
- Кадр: `u32` big-endian длина + JSON-сообщение (`serde_json`). Максимум 8 MiB. JSON
  достаточен: сообщения маленькие, библиотека идёт страницами, отлаживать текст проще.
- Версия протокола — `u32` в `Hello`; несовпадение → `Reject` и выход.
- Один писатель (backend) и один читатель (UI) на соединение. Pipe сохраняет порядок, пока
  соединение живо.
- Нет heartbeat, seq, подтверждений доставки и повторной отправки. Обрыв pipe — это конец
  сессии: UI показывает ошибку и выходит, backend продолжает работу. Новое окно = новый
  `Hello` и новый `State`.
- Запись кадра с таймаутом 5 с: медленный UI отключается, а не блокирует backend.
- `scope` — `MEDIAA2_INSTANCE_ID` для тестовых экземпляров, иначе `default`.

### 4.2. Весь протокол

```rust
pub const PROTOCOL: u32 = 1;

pub enum ClientMessage {
    Hello { protocol: u32, role: Role, startup: Option<UiStartup> },
    Command { id: u64, command: ClientCommand },
    Ui(UiEvent),
}

pub enum Role { Launcher, Ui }

pub enum UiStartup { Show, Tray, Picker { session: u64 } }

pub enum ServerMessage {
    State(State),
    Result { id: u64, result: Result<CommandOk, CommandError> },
    UiRequest(UiRequest),
    Preview(PreviewReady),
    Reject { reason: RejectReason },
}

pub struct State {
    pub settings: Settings,
    pub recording: RecordingState,
    pub conversion: ConversionState,
    pub recovery: RecoveryState,
    pub notices: Vec<Notice>,
    pub picker: Option<PickerState>,
    pub library_revision: u64,
}
```

`State` — вся небольшая часть состояния backend. Backend отправляет его сразу после `Hello` и
при каждом изменении, с коалесингом до 10 раз в секунду. UI рисует полученный `State` целиком;
версий, merge и частичных обновлений нет. Настройки меняются редко, но их сериализация — это
единицы килобайт, поэтому отдельный тип обновления не нужен.

Библиотека в `State` не лежит: в ней могут быть десятки тысяч записей. В `State` есть только
`library_revision`; сами записи UI запрашивает страницами (раздел 4.3).

Команды:

```rust
pub enum ClientCommand {
    ShowWindow,
    Quit,
    SetSettings(Settings),
    HotkeysPaused(bool),
    Recording(RecordingCommand),
    Conversion(ConversionCommand),
    Screenshot(ScreenshotCommand),
    Picker(PickerCommand),
    Library(LibraryCommand),
    Preview { path: PathBuf, kind: PreviewKind },
    Probe { path: PathBuf },
}

pub enum CommandOk {
    Unit,
    Accepted,
    Shot(ShotResult),
    PickerStarted { session: u64 },
    PickerFrame { path: PathBuf, width: u32, height: u32 },
    LibraryPage(LibraryPage),
    Preview(PreviewState),
    Probe(MediaInfo),
}

pub struct CommandError { pub code: ErrorCode, pub message: String }

pub enum ErrorCode { Busy, Invalid, NotFound, Unavailable, Cancelled, Internal }
```

Запросы backend → UI:

```rust
pub enum UiRequest {
    ShowWindow,
    HideWindow,
    OpenPicker { session: u64 },
    Quit,
}

pub enum UiEvent {
    Ready,
    WindowShown,
    WindowHidden,
    Closing,
}

pub struct PreviewReady { pub path: PathBuf, pub kind: PreviewKind, pub file: PathBuf }
```

Правила:

- Команда — быстрый запрос. Старт записи/конвертации отвечает `Accepted`, дальше состояние
  приходит в `State`. Быстрые команды (probe, снимок, commit, страница библиотеки) отвечают
  результатом.
- Таймаут быстрой команды — 15 с; у `Accepted` таймаута нет, прогресс виден в `State`.
- Отмена — `Recording(Cancel*)`, `Conversion(Cancel)`, `Picker(Cancel)`. Поздний результат
  отсекается по `job`/`session` внутри состояния.
- `PickerCommit` и `PickerCancel` — обычные команды, а не `UiEvent`: backend возвращает
  `ShotResult`.
- `Preview` — отдельное сообщение, потому что превью может готовиться секунды; команда
  отвечает `Ready`/`Pending`, а `Preview { path, kind, file }` приходит, когда файл готов.
- UI создаёт окно скрытым и присылает `Ready`; backend показывает его только после этого.

### 4.3. Библиотека по запросу, а не потоком

```rust
pub enum LibraryCommand {
    Query { search: String, kind: Option<MediaKind>, sort: Sort, offset: u32, limit: u32 },
    Refresh,
    Rename { from: PathBuf, to: PathBuf, title: String },
    Delete { paths: Vec<PathBuf>, permanent: bool },
}

pub struct LibraryPage {
    pub revision: u64,
    pub total: u32,
    pub entries: Vec<LibraryEntry>,
    pub next_offset: Option<u32>,
}
```

- Все SQL-запросы — в `storage`; backend только передаёт `Query`. Поиск — простой
  case-insensitive `LIKE` по title/name; подсказка в UI должна соответствовать, без
  необещанных `*`/`^`/`&&`.
- UI загружает первую страницу (200 записей), следующую — при скролле. Держит только
  загруженные страницы, а не всю библиотеку.
- `Refresh` сканирует папку, обновляет БД и увеличивает `library_revision`; UI видит новое
  значение в `State`, сбрасывает загруженные страницы и загружает первую заново.
- Запись, снимок и конвертация добавляют строку сами и увеличивают revision.
- Rename/delete/cleanup меняют файл и БД, затем увеличивают revision.
- Никаких дельт, чанков, `Ready`-маркера библиотеки и полного списка в памяти UI.

Это компромисс: один лишний запрос при скролле, зато протокол не знает про размер библиотеки.

### 4.4. Чего в протоколе нет

- `Welcome`, `Reset`, `Subscribe`, маркера `Ready` для библиотеки, чанков и дельт.
- `StateHub` с версиями на store.
- `EventLog`, `seq`, `Resync`, `Ping/Pong`, heartbeat.
- `name: String` + `payload: Vec<u8>`.
- Отдельных `*Wire`-типов и парных `encode_*`/`decode_*`.
- Второго соединения для capture.
- Строковых ключей состояния.

### 4.5. Почему это корректно

| Ситуация | Механизм |
|---|---|
| UI закрыли во время записи | backend владеет сессией; новое окно получает актуальный `State` |
| UI закрыли во время picker | backend видит disconnect и отменяет сессию; поздний commit отклоняется по `session` |
| backend упал | UI получает ошибку чтения, показывает диалог и выходит; запись восстанавливается при старте |
| два «Открыть окно» подряд | presence в `Starting` не запускает второй процесс; в `Open` шлёт `ShowWindow` |
| второй UI подключился | backend отвечает `Reject{UiAlreadyOpen}`, существующему шлёт `ShowWindow` |
| повторный старт записи | reducer в `Active` возвращает `Rejected`; UI показывает ошибку |
| поздний `Finished` старой записи | `job` не совпадает с текущим — no-op |
| UI не успевает читать | таймаут записи закрывает соединение; окно переоткрывается |
| библиотека изменилась | `library_revision` в новом `State`; UI сбрасывает страницы и грузит первую |
| команда пришла раньше первого `State` | UI сначала запускает читатель, потом отправляет команды |
| настройки изменились | один UI; backend применяет последнюю версию и присылает `State` |

Инвариант: **backend — единственный, кто меняет доменное состояние.** UI не делает
оптимистичных переходов, кроме локального черновика настроек и hover/выделения.

## 5. Backend

### 5.1. Tokio-модель

```text
backend::run
├── IpcListener::accept
│   └── connection task (State, results, UiRequest, commands)
├── StateStore (единственный State + watch<Arc<State>>)
├── SettingsService
├── LibraryService
├── PreviewService
├── RecordingService
├── ConversionService
├── ScreenshotService
└── PresenceService (tray, hotkeys, UI process, mask, outline)
```

- Каждый сервис — задача с `mpsc::Receiver<Cmd>` и `CancellationToken`.
- `StateStore` — короткая критическая секция: сервис меняет свои поля и получает новый
  `Arc<State>`; `watch` хранит только последнюю версию. Отдельного event log и подписок нет.
- Connection task на каждое изменение `watch` отправляет `State`; несколько изменений
  подряд коалесятся в один кадр.
- Долгая работа — `spawn_blocking`; прогресс — `mpsc`/`watch`, не `AtomicI32`.
- `JoinSet` в composition root; завершение по `CancellationToken`.
- Никаких глобальных `Mutex`/`static`; SQLite-соединение живёт в одном сервисе.
- Тикеры: запись — 100 мс, конвертация — 250 мс, уведомления — по событию.

### 5.2. Composition root

```rust
pub struct Backend {
    state: StateStore,
    settings: SettingsHandle,
    library: LibraryHandle,
    recording: RecordingHandle,
    conversion: ConversionHandle,
    screenshots: ScreenshotHandle,
    presence: PresenceHandle,
    notices: NoticeHandle,
    tasks: JoinSet<()>,
}
```

Никаких 44 полей. Всё, что нужно сервисам, передаётся в конструктор явно. `StateStore` — это
`Mutex<State>` + `watch::Sender<Arc<State>>`; блокировка держится только на копирование полей,
без I/O и await.

### 5.3. Домены и чистые переходы

Правило: `fn apply(state, event, now) -> (state, Vec<Effect>)`. Без I/O, `Instant::now`,
файлов и окон.

#### Запись

```rust
enum Recording {
    Idle,
    Starting { job: JobId, target: Target, started: Instant },
    Active {
        job: JobId,
        target: Target,
        started: Instant,
        paused: Option<Instant>,
        sources: Vec<String>,
        has_audio: bool,
    },
    Finalizing {
        job: JobId,
        recorded: Duration,
        progress: Option<f32>,
        discard: bool,
    },
}
```

| Состояние | Событие | Новое | Эффекты |
|---|---|---|---|
| Idle | Start | Starting | SpawnCapture |
| Starting | Started | Active | — |
| Starting | StartFailed | Idle | Notify, Tray |
| Starting | Cancel | Idle | CancelJob |
| Active | Pause | Active(paused) | PauseCapture, Tray |
| Active | Resume | Active | ResumeCapture, Tray |
| Active | Stop(Save) | Finalizing | StopCapture, Finalize |
| Active | Stop(Discard) | Finalizing | StopCapture, Discard |
| Active | Tick | Active | — |
| Finalizing | Progress | Finalizing | — |
| Finalizing | Finished | Idle | AddLibrary, CopyFiles, Sound, Notify, Tray |
| Finalizing | Failed | Idle | Notify, Tray |
| Finalizing | Cancel | Finalizing(discard) | CancelJob |
| любое | Session(job ≠ current) | без изменений | — |

Эффекты — значения; сервис их исполняет и владеет `JoinHandle`. `Finalizing` не освобождает
результат до терминального события.

#### Presence

```rust
enum Presence {
    NoUi,
    Starting { mode: UiStartup, attempts: u8, since: Instant },
    Open { visible: bool, show_on_ready: bool },
    Picker { session: u64, previous_visible: bool, ready: bool },
    Quitting,
}
```

| Состояние | Событие | Новое | Эффекты |
|---|---|---|---|
| NoUi | OpenRequested | Starting | Spawn(Show) |
| Starting | UiConnected(Show) | Open(visible=false, show_on_ready=true) | — |
| Starting | UiConnected(Tray) | Open(visible=false, show_on_ready=false) | — |
| Starting | UiConnected(Picker) | Picker(ready=false) | — |
| Picker | UiReady | Picker(ready=true) | Send(OpenPicker) |
| Open | UiReady | Open(visible=show_on_ready, show_on_ready=false) | Send(ShowWindow), если show_on_ready |
| Starting | UiGone | Starting(attempts+1) | Spawn, если attempts < 2; иначе NoUi + Notice |
| Open | HideRequested | Open(visible=false) | Send(HideWindow) |
| Open | ShowRequested | Open(visible=true) | Send(ShowWindow) |
| Open | PickerRequested | Picker | Send(HideWindow), Spawn(Picker) или Send(OpenPicker) |
| Picker | PickerDone | Open(previous) | Send(ShowWindow), если был видим |
| Picker | UiGone | NoUi | CancelPicker |
| Open/NoUi | Quit | Quitting | Send(Quit), FinishQuit |

UI создаёт окно скрытым и сообщает `Ready` после инициализации; backend показывает его только
после `Ready`. Это убирает гонку «первый UI поднялся и вышел» и белый кадр.

Тесты: один клик — один UI; падение при старте — один retry; повторный клик не создаёт второй
процесс; закрытие во время picker отменяет сессию; `ShowWindow` после снимка только если окно
было видимо.

#### Конвертация и picker

- `Conversion`: `Idle → Probing → Ready → Running { job, item, done, total } → Idle`; отмена
  переводит в `Cancelling`, поздний результат отбрасывается по `job`.
- `Picker`: `Idle → Capturing → Selecting { session, frames } → Committing → Idle`; отмена и
  timeout освобождают кадры; commit проверяет `session`.

### 5.4. Сервисы

| Сервис | Состояние | Эффекты |
|---|---|---|
| settings | `Settings` + writer | atomic TOML, hotkeys, tray, language |
| library | SQLite + проверка путей | add/verify/refresh, rename/delete, revision |
| previews | очередь и файлы | thumbnail/hover/proxy, TTL cleanup |
| recording | `Recording` + jobs | media, library, clipboard, sound, notices |
| conversion | `Conversion` + jobs | media, library, notices |
| screenshots | picker sessions + jobs | capture, render, save, clipboard, library |
| presence | `Presence` + tray view | spawn UI, show/hide/quit, tray, hotkeys, mask, outline |
| notices | ring 32 + TTL | publish |

Трей и автозапуск принадлежат backend: иконка и меню выводятся из `Recording`,
`Conversion` и настроек, а не приходят из UI (`tray_state` больше нет). Автозапуск
настраивается backend-ом при изменении `launch_at_login`; UI только показывает поле.

Библиотека:

- SQLite — авторитетный список результатов приложения, а не кеш произвольной папки.
- Запись добавляется **только после успешной публикации файла**: запись, снимок, конвертация,
  recovery. Это исправляет текущую дыру, где выход конвертации не индексируется, а снимок
  индексирует UI.
- Старт: открыть БД; `user_version` не совпал — удалить и создать заново; удалить строки,
  чьих файлов больше нет. Полного скана на старте нет — запуск не зависит от размера папки.
- Если БД пуста (первый запуск/старое дерево) — один скан папки, чтобы подхватить уже
  существующие результаты. Дальше — только явная команда `Library(Refresh)`.
- `Refresh`: скан папки (не рекурсивный, без `.cache`), diff по path/mtime/size, добавление
  **только файлов MediaA** (имя парсится `naming::parse_path`) и медиа-расширений.
- Rename/delete: команда backend, файл + БД, `library_revision += 1`; удаление в корзину.
- Никакого `notify`-watcher, периодического rescan, overflow-логики и thumbnail atlas.
- Превью — отдельные файлы в `.cache/previews/`; при удалении/renaming файла кэш чистится.
- `LibraryEntry`: `path`, `name`, `title`, `kind`, `size`, `modified`, `taken`, `details`.
  Один тип и один поиск/сортировка; лимит страницы один (200).

Preview:

- `key = hash(path, mtime, kind)`; файл есть — готово; нет — в очередь.
- Дедупликация в памяти; TTL 7 дней; при выключении animated/full previews файлы удаляются.
- UI получает путь и грузит текстуру; backend не гоняет пиксели по IPC.

Настройки:

- Единственный писатель — backend. UI присылает `SetSettings` на каждое изменение.
- Writer коалесит записи (300 мс) и пишет атомарно; ошибка — уведомление.
- `SetSettings` возвращает применённые настройки; UI обновляет черновик, если он не грязнее
  ответа.

### 5.5. Завершение и восстановление

- `Quit` из трея: `Presence::Quitting` → `UiRequest::Quit` → ждать disconnect ≤ 2 с →
  `CancellationToken` всем сервисам → writer.flush (2 с) → выход.
- Активная запись при выходе: остановить захват и оставить work dir; recovery финализирует
  при следующем старте. Это проще, чем ждать финализацию в UI-потоке, и не теряет файл.
- Recovery — первая задача recording-сервиса при старте, до UI.
- Падение backend не блокирует UI: соединение рвётся, UI показывает диалог.

## 6. Frontend

### 6.1. Состояние

```rust
struct App {
    conn: Connection,
    view: BackendView,
    ui: UiState,
    textures: Textures,
}
```

- `BackendView`: текущий `State` (settings, recording, conversion, recovery, notices, picker,
  `library_revision`) плюс загруженные страницы библиотеки; обновляется одной функцией
  `apply(State)`; тестируется без egui.
- `UiState`: страница, навигация, `LibraryPage`, `RecordPage`, `AudioPage`, `ConvertPage`,
  `SettingsPage`, `CapturePage`, `viewer`, `picker`, `modals`.
- Страница получает `&mut PageState`, `&BackendView` и `&mut Actions`; не получает весь `App`.
- `Actions`: `send(ClientCommand)`, `notice(...)`, `open/reveal`, `dialog`.
- Нет `Intent`, `AppCommand`, `Ctx` с 26 полями и `handle_intents`.

### 6.2. Клиент

- Отдельный поток с `tokio` current-thread runtime; UI общается через каналы.
- Читатель применяет `State`/`Result`/`UiRequest`/`Preview`; вызывает
  `ctx.request_repaint()`.
- Писатель отправляет команды и хранит ожидающие `id` с таймаутами.
- Обрыв: диалог «backend недоступен», выход. UI не перезапускает backend.

### 6.3. Страницы

| Страница | Состояние | Команды |
|---|---|---|
| Library | query/sort/filter (server-side), loaded pages, selection, scroll, viewer | `Library(Query/Refresh/Rename/Delete)`, `Preview`, `Conversion(Start)` |
| Screenshot | target, monitor/window, after-actions | `Screenshot(Monitor/Window)`, `Picker(Start)` |
| Record | target, sources, profile draft | `Recording(Start/Stop/Pause/Cancel)` |
| Record audio | format/bitrate/mic/system | `Recording(Start/Stop)` |
| Convert | files, target, options, output | `Probe`, `Conversion(Start/Cancel)` |
| Settings | draft, advanced, dialogs | `SetSettings` |
| About | версия, ссылки | — |
| Picker | кадр, selection, shapes, tool, text | `Picker(Monitor/Commit/Cancel)` |

UI не вызывает `std::fs` для медиа, не открывает БД, не запускает FFmpeg и не делает снимки.
Разрешены: диалоги выбора файлов/папок, `open`/`reveal`, текстовый буфер, чтение кадра viewer
и звук.

### 6.4. Выбор области

1. Backend получает `Picker(Start)` (из UI, трея, хоткея или `--area`).
2. Backend снимает монитор в PNG в work dir, создаёт `PickerSession { id, monitor, image, scale }`.
3. Если UI нет — запускает `--ui --picker <id>`. Если есть — `UiRequest::OpenPicker`.
4. UI показывает overlay: кадр, selection, инструменты, аннотации, `Tab` — следующий монитор
   (`Picker(Monitor)` возвращает PNG).
5. `Picker(Commit { selection, shapes, format, action })` → backend режет, рисует аннотации,
   сохраняет, кладёт в библиотеку и буфер, возвращает `ShotResult`.
6. `Picker(Cancel)` или закрытие процесса → backend освобождает сессию.

Никаких RGBA-файлов туда-обратно, `CaptureClient`, `commit`-протокола и prewarm viewport.
Окно MediaA исключается из захвата через `WDA_EXCLUDEFROMCAPTURE`; если на целевой Windows
это не сработает — fallback: скрыть UI и подождать 250 мс (проверить в M2).

### 6.5. Viewer

- UI декодирует файл через `mediaa2_media::preview` (read-only), звук — через
  `platform::audio`.
- Backpressure: не более одного кадра в работе; при отставании кадр пропускается.
- Hover- и thumbnail-файлы готовит backend; UI только грузит изображения.

### 6.6. i18n

- `mediaa2-i18n`: `locales/en.toml`, `locales/ru.toml`, `Language`, `set_language`, `t!`,
  `languages()`.
- Сейчас 865 ключей и 2 языка; после удаления агента остаётся около 410.
- Backend использует тот же крейт для трея и уведомлений; язык берёт из настроек.
- Тесты: совпадение ключей, отсутствие неизвестных ключей, переключение языка,
  склонения (`i18n::files`).
- Никаких строк интерфейса вне locale-файлов; никаких locale-файлов вне `i18n`.

## 7. Данные

### 7.1. Настройки

- Файл тот же: `~/.mediaa.toml`.
- Схема та же, минус секция `agent` и техническое `record.encoder_tuning`; `#[serde(default)]`,
  неизвестные поля игнорируются.
- Секции и поля сохраняются для паритета UI: appearance (темы System/Light/Dark + Lime/Peach),
  startup, previews, window/tray, folders, hotkeys, три профиля записи (`record`, `hotkey_*`,
  `region_*`) с их `anim`, `saved_formats`, `advanced_settings`, `audio_record`,
  `hotkey_screen`.
- Простой/расширенный режим остаётся: это видимая часть UI. Внутри новый код убирает только
  расхождение `load()`/`sanitized()`: нормализация применяется всегда и одинаково, а не
  зависит от того, открывали ли advanced.
- `encoder_tuning` больше не хранится: `zerolatency = true`, `threads = None`,
  `lookahead = None` — это дефолт, который и так даёт 102 MB вместо 794 MB на 1080p30.
- Значения по умолчанию сохраняются: MKV 60/1080/85, GIF для региона 30/720/256/Bayer,
  MP3 256, `close_to_tray = false`, хоткеи выключены, `tray_left_click = ScreenArea`,
  `viewer_volume = 1.0`, превью включены.
- Запись только backend, атомарная, с коалесингом.
- Старый файл читается; при сохранении секция агента исчезает. Во время параллельной
  разработки новый экземпляр использует `MEDIAA2_SETTINGS`, чтобы не портить настройки
  старого приложения.
- Объединение трёх профилей в один — возможное продуктовое упрощение позже, не в v1: это
  изменило бы экран настроек.

### 7.2. Библиотека

SQLite `.cache/library.db`, единственный писатель — backend.

```sql
PRAGMA journal_mode = WAL;
PRAGMA user_version = 1;

CREATE TABLE entries (
    path      TEXT PRIMARY KEY,
    kind      TEXT NOT NULL,
    size      INTEGER NOT NULL,
    modified  INTEGER NOT NULL,
    taken     TEXT,
    title     TEXT,
    width     INTEGER,
    height    INTEGER,
    duration  REAL,
    audio     INTEGER
);

CREATE INDEX entries_taken ON entries(taken DESC);
CREATE INDEX entries_modified ON entries(modified DESC);
CREATE INDEX entries_title ON entries(title COLLATE NOCASE);
CREATE INDEX entries_size ON entries(size DESC);
CREATE INDEX entries_kind ON entries(kind);
```

- Нет таблицы thumbs, счётчиков и atlas.
- `user_version` не совпал → файл удаляется, скан пересоздаёт. Пользовательские файлы не
  трогаются.
- Все SQL-строки — только в `mediaa2-storage`.

### 7.3. Файлы и кэш

| Что | Где |
|---|---|
| медиатека | `Documents/MediaA` (настройка) |
| БД | `<медиатека>/.cache/library.db` |
| превью | `<медиатека>/.cache/previews/<hash>-<kind>.<ext>` |
| работа записи | `%LOCALAPPDATA%/MediaA/work` |
| логи | `%LOCALAPPDATA%/MediaA/logs` |
| настройки | `~/.mediaa.toml` |

- Имена результатов, конфликты и атомарная публикация — из `core` (перенос текущих правил).
- Запись/снимок/конвертация не перезаписывают существующий файл.
- TTL превью — 7 дней, уборка при старте и раз в сутки.

## 8. Медиа и платформа

### 8.1. Переносим почти как есть

| Код | Строк | Что делаем |
|---|---:|---|
| `media/recorder` | ~1 100 | сессия, захват, пауза, finalize, recovery, несколько источников |
| `media/convert` | ~700 | матрица конвертаций, прогресс, отмена |
| `media/filters` | ~200 | строки фильтров |
| `gif` + `webp` + `ffmpeg` | ~1 600 | в модули `media` |
| `render/annotate` | ~700 | в `media`; финальный crop и аннотации |
| `media/preview` | ~600 | чтение кадров, звук, прокси |
| `platform/screen`, `windows/wgc` | ~600 | захват монитора/окна, WGC |
| `platform/window`, `outline`, `cursor`, `mask` | ~700 | окна, контур записи, курсор |
| `platform/audio` | ~430 | cpal, системный звук |
| `platform/clipboard`, `files`, `startup`, `keys` | ~200 | обёртки |
| `backend/settings_file` | ~210 | коалесящий writer с flush и ошибками |
| `backend/recording/plan` | ~300 | чистое планирование источников и целей |
| `backend/clipboard` | ~340 | один воркер, таймауты, восстановление |
| `backend/preview` | ~450 | lazy-очередь, 1 воркер, TTL |
| `core/settings`, `formats`, `geometry`, `paths`, `naming`, `atomic` | ~2 500 | почистить от агента |

`storage/library_index` и `storage/thumb_cache` в живом приложении не используются (только
benches/tests): их не переносим. Живой `LibraryStore` даёт SQL-поведение, но его
версионирование и drop-эвристику переписываем.

### 8.2. Переписываем

- `protocol`, `sync`, `storage/index`, `thumb_cache` — заменяются новыми.
- `backend/daemon`, `dispatch`, `services`, `features` — заменяются сервисами и доменами.
- `frontend/app`, `features`, `infra`, `ui/*` — заменяются новой оболочкой и страницами;
  отрисовка страниц переносится с упрощением.
- `app` — роли и лаунчер.
- Linux/macOS-код в v1 не входит; `platform` сохраняет доменный API, чтобы добавить позже.

### 8.3. Запись

- Вход: `RecordingRequest { target, output, fps, quality, height, audio, mic, system, tuning }`.
- Источники: экран/окно/несколько окон/аудио; capture-потоки и pacer — из старого `media`.
- Выходы: MP4/MKV (H.264 + AAC), GIF, animated WebP, аудиоформаты.
- `zerolatency`, потоки/lookahead, two-pass GIF, WebP Quality/Fast — сохраняются.
- Несколько окон — отдельный файл на окно, выравнивание длительности.
- Пауза/резюме, отмена, восстановление, минимальная длительность — сохраняются.
- Прогресс и фазы — через состояние backend, не через UI-контроллер.

### 8.4. Конвертация

- Вход: список файлов + `ConvertOptions` (target, quality, height, audio, output dir/name).
- Очередь последовательная, как сейчас; ошибка одного файла не останавливает пакет.
- Probe, оценка не нужны (в текущем UI их уже нет); прогресс/отмена — обязательны.
- FFmpeg-job, фильтры и кодеки — из старого `media/convert` + `filters`.

### 8.5. Снимки

- Backend: capture → `Frame`.
- `media::screenshot::compose(frame, selection, shapes) -> Frame` (crop + аннотации).
- Сохранение PNG/WebP, копирование изображения/файла, запись в библиотеку.
- UI: overlay и аннотации; финальный рендер не в UI.

## 9. Тесты

### 9.1. Уровни

| Уровень | Что проверяет | Где |
|---|---|---|
| unit | настройки, геометрия, форматы, имена, атомарная запись | `core` |
| protocol | roundtrip сообщений, handshake, версия, `State`, страницы библиотеки | `protocol` |
| domain | таблицы переходов, invalid, stale, идемпотентность, время | `backend/domain` |
| storage | схема, CRUD, пересоздание, конкурентный доступ | `storage` |
| services | писатель настроек, refresh/диф, preview-очередь, picker-сессия | `backend/services` |
| frontend | `BackendView::apply`, page-модели, kittest-экраны | `frontend` |
| media | реальные MP4/GIF/WebP, конвертации, recovery | `media`, только по запросу |
| e2e | процессы, трей, снимок, запись, конвертация | Windows smoke |

### 9.2. Обязательные тесты переходов

- Presence: 15 сценариев (один запуск, retry, повторный клик, quit, picker, disconnect).
- Recording: 20 сценариев (start/stop/pause/cancel, stale job, finalize, failure, recovery).
- Conversion: 12 сценариев (очередь, ошибка файла, отмена, поздний результат).
- Picker: 10 сценариев (commit, cancel, tab, timeout, stale session).
- Settings: sanitize, reset, смена папки/языка/хоткеев.

Тест не требует Windows, окна, трея и `sleep`: время — параметр, эффекты — значения.

### 9.3. Протокол и IPC

- `tokio::io::duplex` вместо pipe: `Hello`/`State`, reject версии, команды и результаты,
  страницы библиотеки, `UiRequest`, `Preview`, обрыв.
- Отдельно: реальный named pipe smoke на Windows (один тест, `#[ignore]`).

### 9.4. UI

- `BackendView::apply` — без egui: настройки, запись, конверсия, уведомления, revision
  библиотеки; страницы библиотеки проверяются отдельно.
- `egui_kittest` — ключевые экраны: Library, Record, Convert, Settings, About, notices,
  picker, модалки.
- Снапшоты: старые 76 не переносятся; новые baselines создаются после стабилизации.
- Interaction/geometry: кнопки старта, блокировка при записи, Tab в picker, закрытие модалок.

### 9.5. E2E smoke

1. Автозапуск backend без окна; трей присутствует.
2. «Открыть окно» → ровно один UI; повторный клик фокусирует его.
3. Закрытие окна → UI завершился, backend жив.
4. «Выход» → оба процесса завершились.
5. Запись 3 с из трея без UI → файл в библиотеке.
6. Снимок области → файл + буфер.
7. Конвертация одного файла → файл в библиотеке.
8. Recovery после kill -9 во время записи.

## 10. Бюджет и правила

### 10.1. Бюджет кода

| Часть | Сейчас (без агента) | Цель |
|---|---:|---:|
| core | ~4 700 | 3 800 |
| i18n | 0 | 150 + locale |
| protocol + sync | 2 556 | 700 |
| storage | 2 003 | 700 |
| platform | 4 523 | 3 400 |
| media (+ render/gif/webp/ffmpeg) | ~9 000 | 6 000 |
| backend | ~6 800 | 3 200 |
| frontend | ~20 600 | 9 000 |
| app | 388 | 150 |
| **production** | **~50 000** | **~27 000** |
| тесты | ~15 000 | ~8 000 |

Цифры — бюджет, а не обещание. Если крейт выходит за бюджет, это повод упростить, а не
добавить слой.

### 10.2. За счёт чего меньше строк

- Агент: минус ~35 тысяч строк крейтов и ~455 locale-ключей; из `Settings` уходит `agent`.
- `sync` + строковый протокол: минус ~2,5 тысячи строк; остаётся один типизированный контракт.
- `*Wire`-дубли, `Intent`, `AppCommand`, `Ctx`-маршаллинг и `ViewState`: минус несколько
  тысяч строк; страницы работают с доменными типами и `Actions`.
- `storage/library_index` + `thumb_cache`: минус ~1,5 тысячи строк; в живом приложении они
  не использовались.
- Три пути capture и `CaptureClient`: минус клиент, RGBA-файлы, commit-протокол.
- Дубли `ipc.rs`, `icon.rs`, `recording_from_state`: один экземпляр каждого.
- Нет compatibility-слоёв, миграций БД, legacy settings и «временных» адаптеров.
- Тесты становятся targeted: переходы, протокол, storage, ключевые экраны; 76 старых
  снапшотов не тянутся.

### 10.3. Правила `crates2/AGENTS.md`

1. Девять крейтов. Новый крейт — только с удалением другого или явным решением пользователя.
2. Зависимости только по схеме раздела 3.4.
3. Backend — единственный владелец доменного состояния и эффектов.
4. Один reducer на домен: `apply(state, event, now) -> (state, Vec<Effect>)`.
5. Никаких `bool`, кодирующих фазу; никаких `Option<Job>` вместо состояния.
6. Время — параметр; `Instant::now()`/`sleep` в переходах запрещены.
7. Протокол — типизированные enum + JSON; никаких строковых команд и `Vec<u8>` payload.
8. Доменные типы сериализуются напрямую; `*Wire`-дублей нет.
9. SQL — только в `storage`; locale — только в `i18n`.
10. UI не пишет файлы, не открывает БД, не запускает медиа. Исключения: viewer (чтение),
    диалоги, open/reveal, текстовый буфер.
11. Комментарии в коде запрещены, кроме `SAFETY`/tool directives.
12. Обратной совместимости нет; неиспользуемое удаляется сразу.
13. Каждый переход, протокол и SQL-путь покрыт тестом без Windows.
14. Windows smoke — отдельно, после зелёных unit-тестов.

## 11. Этапы

| Этап | Содержание | Готово, если |
|---|---|---|
| M0 | `crates2` workspace, AGENTS, README, скрипты сборки/тестов | `cargo check` и `cargo test` пустого каркаса |
| M1 | core, i18n, protocol, storage | тесты схемы, ключей, roundtrip |
| M2 | platform + media: перенос и чистка; spike capture exclusion | 3-секундная запись, конвертация, снимок в тестах |
| M3 | backend: settings, library, previews, IPC; UI-заглушка | headless-клиент получает `State` и команды |
| M4 | frontend: оболочка, Library, Settings, About | окно работает с backend, kittest ключевых экранов |
| M5 | conversion | пакет и отмена из UI |
| M6 | recording + audio | запись/пауза/стоп/восстановление из UI и трея |
| M7 | screenshots + picker | область/экран/окно, аннотации, буфер |
| M8 | presence, tray, hotkeys, launcher, quit | e2e smoke 1–5 |
| M9 | паритет, удаление старого, документация | чек-лист функций и зелёный smoke |

Порядок выбран так, чтобы каждая следующая часть опиралась на уже проверенный контракт.
M2 начинается со spike: если `WDA_EXCLUDEFROMCAPTURE` не работает на целевой Windows,
возвращаемся к fallback «скрыть UI + 250 мс» до начала M7.

## 12. Риски

| Риск | Почему | Что делаем |
|---|---|---|
| Паритет UI | 20k строк страниц, 76 снапшотов | переносим экраны по одному, сверяем скриншотами; агентские экраны не переносим |
| Задержка picker без резидентного UI | холодный старт egui | маска backend + кадр уже снят; spike в M2; при необходимости — отдельный лёгкий picker-процесс |
| Исключение окна из захвата | зависит от Windows/WGC | проверяем в M2; fallback hide + 250 мс |
| FFmpeg в frontend | viewer требует декод | только read-only; запись/конвертация в backend |
| Библиотека и внешние файлы | БД хранит результаты приложения; чужие файлы не появляются сами | старт удаляет только мёртвые строки; `Refresh` сканирует папку явно; старый UI-путь не регрессирует |
| Clipboard может зависнуть | ОС | отдельный поток + timeout + перезапуск, как сейчас |
| Трей/хоткеи | текущая больная зона | presence как чистый reducer + Windows smoke |
| Потеря настроек при параллельном запуске | два exe | отдельный `MEDIAA2_SETTINGS` в разработке |
| Размер состояния | библиотека на 50k записей | в `State` только `library_revision`; записи — страницами по 200 |
| Бюджет LOC | амбициозный | бюджет — ориентир; сложность оправдывается только тестом/риском |

## 13. Что не делаем

- Не переносим агента, чаты, каталоги, модели, очередь, eval. Когда агент понадобится, он
  добавляется отдельным backend-сервисом, новыми командами/обновлениями протокола и новыми
  страницами UI; ядро, storage и медиа для этого менять не нужно.
- Не добавляем новые функции и не меняем дизайн UI.
- Не поддерживаем Linux/macOS в v1.
- Не делаем миграции БД: кеш пересоздаётся.
- Не добавляем DI, event bus, plugin-архитектуру, второй event loop.
- Не даём UI доступ к БД/FFmpeg-записи.
- Не оставляем резидентный скрытый UI.
- Не добавляем файловый watcher, периодический rescan и thumbnail atlas.
- Не тянем старые `*Wire`/`Intent`/`StateHub` ради совместимости.

## 14. Что прошу подтвердить

1. **Windows-only v1** — рекомендуются; Linux/macOS-код не переносим, но `platform`
   оставляем доменным.
2. **Один exe с ролями** — рекомендуется; отдельные `mediaa-backend.exe`/`mediaa-ui.exe` не
   нужны.
3. **Закрытие окна при `close_to_tray = true` завершает UI-процесс** (backend остаётся в
   трее) — это соответствует описанию «frontend запускается по требованию». При
   `close_to_tray = false` закрытие завершает и backend.
4. **Capture выполняет backend**, UI только выбирает и рисует аннотации — рекомендуется;
   это убирает `CaptureClient` и RGBA-файлы.
5. **Убрать thumbnail atlas** и хранить превью отдельными файлами — рекомендуется.
6. **Оставить viewer-декод в UI** (read-only) — иначе придётся гонять кадры по IPC.
7. **Сохранить простой/расширенный режим и три профиля записи** — это видимая часть
   текущих настроек; объединение в один профиль откладываем.
8. **Библиотека — БД результатов приложения + явный `Refresh`**, без watcher и
   автоматического импорта чужих файлов — рекомендуется.
9. **Начинать с M0–M1** после подтверждения структуры.
