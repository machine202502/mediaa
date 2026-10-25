# UI/UX regression tests with mocked devices

## Цель

До упрощения runtime-кода получить пользовательский регрессионный контур, который:

- запускает настоящие экраны и контроллеры MediaA без native capture;
- подставляет детерминированные дисплеи, окна и микрофоны;
- проверяет доступные элементы, действия, состояния и ограничения через `egui_kittest`;
- фиксирует ключевую компоновку snapshot-тестами;
- позволяет безопасно менять WebP lifecycle, capture lifecycle и настройки после появления
  тестов.

## Выбранный инструмент

Использовать уже подключённый `egui_kittest 0.36.2`, а не собственный UI harness.

Почему он подходит:

- `Harness` запускает настоящий egui UI;
- `kittest` ищет элементы через AccessKit и позволяет пользовательские действия;
- `Harness::render` и snapshot API проверяют визуальный результат;
- поддерживаются фиксированные размеры, тема, OS, шаги и стабильное время;
- для проверки паники, доступности и визуальной регрессии не нужны окна или реальные устройства.

Проект уже использует `Harness` в тестах записи, библиотеки и уведомлений. Новый слой будет
только тестовой сборкой `App` и повторно используемыми fixtures.

## Тестовое состояние

Тестовая фабрика создаёт `App` без пользовательских сервисов:

- отдельная временная библиотека;
- без tray, global hotkeys и native window;
- no-op `SettingsWriter`;
- пустые фоновые задачи;
- подставленные `monitors`, `windows`, `mic_devices` и default microphone;
- фиксированные locale и тема;
- изолированный egui context на каждый тест.

Каталог устройств задаётся данными, а не новым production trait:

```text
MonitorFixture:
  primary 1920x1080
  secondary 2560x1440

WindowFixture:
  короткое имя
  длинное имя приложения и окна

MicrophoneFixture:
  default: Headset
  available: Headset, Studio USB Microphone
```

Если `App` начнёт менять способ чтения каталога, меняется одна тестовая фабрика.

## Пользовательские сценарии

### Запись экрана

1. Открыть страницу записи.
2. Увидеть оба mock-дисплея и отметку primary.
3. Выбрать secondary.
4. Убедиться, что выбранный дисплей сохранён в состоянии.
5. Убедиться, что кнопка запуска доступна и заблокирована только по ожидаемым причинам.

### Запись окна

1. Переключиться на источник «Окно».
2. Увидеть mock-окна с названиями и размерами.
3. Выбрать окно.
4. Убедиться, что кнопка запуска доступна и выбранное окно сохранено.

### Микрофоны

1. Увидеть default microphone в выборе.
2. Открыть список и увидеть auto, off и два mock-микрофона.
3. Выбрать `Studio USB Microphone`.
4. Убедиться, что `MicChoice::Device` обновился.
5. Отдельно проверить сохранённый отсутствующий микрофон и состояние без источников.

### Настройки профилей

1. Открыть настройки с mock-дисплеями.
2. Проверить выбор дисплея для быстрой записи.
3. Проверить, что mock-микрофоны доступны во всех редакторах профилей.
4. Проверить, что изменение одного профиля не меняет другой.

### Компоновка

- wide layout не переносит и не обрезает основные controls;
- narrow layout переходит в одну колонку;
- панель источников сохраняет фиксированную высоту;
- кнопка запуска остаётся видимой;
- disabled controls имеют визуально отличимое и доступное состояние.

## Snapshot-тесты

Включить feature `snapshot` у `egui_kittest`.

Базовые снимки:

- запись экрана с двумя mock-дисплеями, dark theme;
- запись аудио с mock-микрофонами, dark theme;
- narrow layout страницы записи;
- настройки с mock-каталогом.

Правила:

- фиксированный размер и `OperatingSystem::Windows`;
- только key states без hover и движущихся индикаторов;
- image snapshots используются для компоновки, а не вместо interaction tests;
- `.diff.png`, `.new.png` и `.old.png` не хранятся в Git;
- baseline обновляется только через `UPDATE_SNAPSHOTS=true`;
- visual snapshots на первом этапе выполняются только на Windows, чтобы системные шрифты не
  делали Linux/macOS baseline ложноположительными; общие AccessKit и geometry tests выполняются
  на всех платформах.

## Реализация

1. Добавить test-only конструктор `App`, который принимает `Settings` и mock-каталог.
2. Добавить fixtures дисплеев, окон и микрофонов в `app/tests` или отдельный тестовый модуль.
3. Подключить `egui_kittest::Harness` с размером окна, Windows OS, dark theme и AccessKit.
4. Написать interaction и geometry tests для записи экрана, окна, аудио и настроек.
5. Включить snapshot feature и создать проверенные Windows baselines.
6. Записать тестовый сценарий в `workflow/practices.md`.
7. Использовать этот набор как входной gate перед принятием task `RUNTIME-SIMPLIFY-*`.

## Затронутые области

| Область | Файлы |
|---|---|
| App test fixture | `crates/app/src/app.rs`, `crates/app/src/app/tests.rs` |
| UI scenarios | `crates/app/src/pages/{record,audio,settings}.rs` и соседние test modules |
| Test dependency | `crates/app/Cargo.toml`, `Cargo.lock` |
| Snapshot config | `kittest.toml`, `.gitignore`, `crates/app/tests/snapshots/` |
| Workflow memory | `workflow/practices.md`, `README.md` при необходимости |

## Проверка

- `cargo test -p mediaa --lib app::tests`
- `cargo test -p mediaa --lib pages::record`
- `cargo test -p mediaa --lib pages::audio`
- `cargo test -p mediaa --lib pages::settings`
- `UPDATE_SNAPSHOTS=true cargo test -p mediaa --lib ui_regression`
- повторный запуск snapshot tests без обновления baseline;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- полный `cargo test --workspace` перед приёмкой.

## Риски

- GPU и системные шрифты могут давать различия snapshots между машинами. Поэтому визуальные
  baseline ограничены Windows, а поведение проверяется AccessKit тестами.
- Тестовая фабрика `App` может стать хрупкой при изменении полей. Она централизуется в одном
  месте и не добавляет production abstraction.
- Реальная запись, native capture и права ОС не проверяются этим контуром; существующие live
  tests остаются отдельным уровнем.
- Слишком широкие snapshots скрывают локальные изменения. Снимок должен показывать конкретную
  страницу и проверяемое состояние при минимальном размере.

## Вне рамок

- реальный захват экрана или воспроизведение через mock-микрофон;
- эмуляция системных permission dialogs;
- замена live platform tests;
- упрощение runtime-кода до получения зелёного UI/UX набора.

## Расширенная матрица приёмки

После первого UI/UX-контура пользователь расширил обязательный набор проверок. Уже
существующие end-to-end тесты не дублируются: новые сценарии проверяют пользовательские
переходы и недостающие комбинации поверх реальных media tests.

| Область | Уже покрыто | Добавить |
|---|---|---|
| Скриншоты | render layout, app screenshot worker, live monitor/window tests | app-level выбор fullscreen/window/region и mocked completion |
| Видео | несколько synthetic sources, MP4/MKV, GIF/WebP, pause/cancel | UI-комбинации источников и форматов, stop через окно и tray command |
| Аудио | все форматы реальным encoder, cancel и device readiness | UI-переходы и отсутствие источников |
| Настройки | значения и несколько отдельных controls | изменение controls и влияние на сформированный запрос/поведение |
| Библиотека | очистка, menu actions, index/search primitives | UI фильтрация и удаление через пользовательский сценарий |
| Конвертация | матрица реальных форматов | UI выбор цели, запуск, progress, cancel и terminal state |
| Компоновка | wide/narrow для записи | несколько размеров для ключевых страниц и отсутствие выхода fixed controls |
| WebP | direct/live/segmented и offline fallback | пользовательский failure notice, trace ID и понятная диагностика |
| Single instance | один guard, второй launch отклоняется | production scope изолирован от dev scope при подмене библиотеки |

### Скриншоты

Проверить пользовательский выбор:

- область экрана;
- весь экран;
- окно из mock-каталога;
- отмену выбора области;
- сохранение/копирование после mocked capture без native desktop.

### Видео и аудио

Для видео проверить UI-состояния полного экрана, нескольких мониторов, нескольких окон и
каждого формата результата. Для аудио проверить страницу, профили микрофона и запуск при
наличии/отсутствии источника.

Отдельно проверить, что stop и cancel из окна и команда трея приводят к одинаковому typed
terminal flow. Реальное кодирование остаётся в media integration tests.

### Настройки, библиотека и конвертация

- Изменение theme, tray policy, screen choice, microphones и recording format должно менять
  состояние, которым пользуется следующий запуск.
- Фильтр типа, поиск, сортировка, меню действий и подтверждение удаления проверяются через UI.
- На странице конвертации пользователь выбирает цель, запускает job, видит progress/cancel и
  terminal result. Реальная матрица форматов остаётся в `media/tests/pipeline`.

### Компоновка

Для страниц screenshot/record/audio/library/convert/settings использовать минимальный,
узкий, обычный и широкий размеры окна. Для каждого размера:

- bottom action не выходит за viewport;
- fixed header/subnav не перекрывает central content;
- split cards переходят в одну колонку без горизонтального выхода;
- основные controls остаются доступны;
- snapshot создаётся только для устойчивого состояния и минимально необходимой области.

### WebP failure и логи

- Сегментированный WebP failure проходит fallback и создаёт пользовательское уведомление.
- Уведомление содержит trace ID и достаточную причину.
- Session event не теряет warning/Notice при fallback.
- Test logger подтверждает, что stage, trace ID и конечная ошибка попали в лог.
- Исходный MKV остаётся доступен, если все варианты WebP завершились ошибкой.

### Single instance

- Обычный запуск не допускает второй процесс с тем же scope.
- Dev-запуск с другим `MEDIAA_DEV_LIBRARY` и `MEDIAA_INSTANCE_ID` не активирует пользовательский
  процесс и не разделяет его библиотеку.
- Повторный запуск внутри одного scope сообщает activation существующему экземпляру.

## Дополнительные задачи

| ID | Смысл |
|---|---|
| `UI-REG-4` | Скриншоты и базовые пользовательские capture flows |
| `UI-REG-5` | Матрица UI для видео/аудио, stop через окно и tray |
| `UI-REG-6` | Действия настроек, библиотеки и конвертации |
| `UI-REG-7` | Responsive geometry и snapshots для всех ключевых страниц |
| `UI-REG-8` | WebP failure notice, fallback и проверка логов |
| `UI-REG-9` | Production/dev single-instance isolation |
| `UI-REG-10` | Полный пользовательский сценарий region editor |
| `UI-REG-11` | Pause/resume/cancel/Saving и WebP notice в состоянии записи |
| `UI-REG-12` | DPI, RU UI и light theme для ключевых страниц |

## Обязательный pre-simplification gate

До удаления `MemoryBudget`, join-watchdog, дублирующих task channels и animation settings должны
быть покрыты:

1. выбор, изменение, аннотирование, сохранение и отмена region screenshot;
2. pause/resume/cancel/save текущей записи и WebP fallback notice с trace ID;
3. Windows DPI 100/125/150/200%, минимальная высота 480 px, Russian UI и light theme.

Остальные расширения — библиотечный поиск, ошибочные состояния конвертации, уведомления и
клавиатурный focus order — входят в следующий, неблокирующий этап.
