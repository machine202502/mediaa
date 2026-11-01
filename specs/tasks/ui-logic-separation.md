# Разделение UI и логики в `crates/app`

## Статус

Целевая архитектура согласована с пользователем 01.11.2026. Код не изменён: документ фиксирует
решение и порядок этапов. Строки этапов — `ARCH-14.1`…`ARCH-14.6` в
[progress.md](../progress.md).

## Цель

Разделить внутри `crates/app` два слоя с явным контрактом: слой UI (нажатия, координаты,
отрисовка) и слой логики приложения (что происходит, кто владеет состоянием и фоновой работой).
Критерий готовности: по сигнатурам модуля видно его слой, не читая остальной крейт; изменение
вёрстки не требует понимания фоновой работы, изменение логики не требует понимания egui.

Пользовательская формулировка задачи: «UI должен быть отдельным модулем, описывающим нажатия и
движение; отдельный слой описывает логику приложения; UI дёргает логику».

## Контекст: текущее состояние

Измерено 01.11.2026.

| Область | Файлов | Строк |
|---|---|---|
| `crates/app/src` всего | 86 | 39 829 |
| `src/app/` (shell + оркестрация + логика capture/recording) | 13 | 7 116 |
| `src/features/` (контроллеры) | 17 | 7 421 |
| `src/pages/` | 26 | 19 512 |
| — из них `src/pages/agent/` | 18 | 15 887 |
| `src/overlay.rs` + `src/overlay/editor.rs` | 2 | 1 999 |
| `src/services/` | 3 | 566 |
| `src/widgets/`, `src/themes/` | 15 | 1 060 |

Факты, на которых стоит решение:

- `App` — 43 поля (плюс 4 test-only); `App::logic()` последовательно опрашивает ~20 источников.
- 37 вхождений `&mut App`, 582 обращения `app.` по крейту; страницы принимают `&mut App` и
  напрямую пишут в `app.settings` и `app.agent.*`.
- Цикл импортов: `features → pages` (`pages::header`, `pages::agent::{AgentUi, Services}`),
  `features → app` (`crate::app::App`, `Page`), `pages → app` (8 файлов), `app → features`.
- Логика разорвана: машина состояний записи в `features/recording`, а старт/сбор/терминал — в
  `app/recording.rs`; job'ы захвата в `features/capture`, а путь capture → save → clipboard →
  notice → показ окна — в `app/capture.rs`.
- `features/capture` владеет `Overlay`; `features/conversion` и `features/library` содержат
  отрисовку (`ui()`, `grid.rs`, `viewer.rs`, `thumbs.rs`).
- `AgentController` реализует `Deref<Target = AgentUi>`, чтобы скрыть совмещение хоста и
  UI-состояния; тип `Services` объявлен в `pages/agent/mod.rs`, а импортируется из `features`.
- Видимость окна решают 4 флага (`hidden_by_us` — 24 упоминания, `recording_quiet` — 21,
  `ghosted` — 7, `quit_when_idle`) и 28 вызовов `show_window`.
- 9+ параллельных action-enum'ов с ручным маппингом в двух местах.
- `settings_changed()` вызывается в 13 местах вручную; дебаунс 600 мс дублирует generation-merge
  в `SettingsWriter`.
- `overlay.rs` — отдельное native-окно + редактор + домен аннотаций + диалог сохранения + запись
  файла, 16 тестов, часть со snapshot'ами.
- Логика агента уже находится в `mediaa-agent`, `mediaa-engine`, `mediaa-queue`. В `crates/app`
  от неё только хост (`features/agent.rs`, 198 строк) и рисование (`pages/agent`, 15 887 строк,
  57 прямых вызовов агентских крейтов).

## Ограничения

- Приложение без пользователей и без обратной совместимости: миграции данных, shim'ы, алиасы,
  deprecated-пути и двойные режимы запрещены ([rules.md](../rules.md)).
- Имена `core` и `platform` заняты крейтами `mediaa-core` и `mediaa-platform`; пересечения в
  именах слоёв не допускаются.
- Логика агента не переносится в `crates/app`: крейты агента — владельцы, приложение рисует.
- `ui_tests.rs` (110 КБ, 120 тестов) и `crates/app/tests/snapshots` остаются контрактом
  поведения: правится только setup и точки входа, сценарии не меняются.
- Внешний вид и поведение не меняются ни на одном этапе: перестройка не является изменением
  продукта.

## Решение

### Слои

```text
crates/app/src/
  main.rs  lib.rs
  shell.rs                    eframe::App: tick → draw → dispatch; единственный egui::Context
  logic/                      логика приложения, без egui
    mod.rs                    AppLogic: композиция владельцев, handle(Intent), tick()
    intent.rs                 enum Intent — единственный вход из UI
    effect.rs                 enum Effect — что нужно сделать окну и выходу
    settings_session.rs       настройки + отложенная запись
    window_presence.rs        вместо hidden_by_us + ghosted + recording_quiet
    notices.rs  jobs.rs  latest_worker.rs
    agent_host.rs             запуск хоста агента, connections/slots, AgentServices
    recording.rs screenshot.rs   чистые решения записи и снимка
    conversion/ library/         контроллеры без отрисовки (ARCH-14.5)
    region_editor.rs            модель и решения редактора области
  ui/                         egui, без решений и эффектов ОС
    chrome.rs nav.rs caches.rs
    pages/{screenshot,record,audio,library,convert,settings,about}.rs
    pages/agent/              как есть: рисование + AgentUi (состояние вида)
    region_editor.rs editor.rs  viewport, ввод, отрисовка, текст
    notices.rs region_marker.rs
    widgets/ themes/
  adapters/                   эффекты ОС, без egui
    clipboard.rs settings_file.rs tray.rs hotkeys.rs desktop.rs dialogs.rs audio.rs
  icon.rs i18n.rs             общие для logic и adapters: иконка трея и тексты уведомлений
```

`icon.rs` и `i18n.rs` остаются в корне крейта, а не в `ui/`: их используют `logic` (тексты
уведомлений) и `adapters` (иконка трея), а `logic` не может зависеть от `ui`.

Идентификаторы `logic`, `adapters`, `shell` в воркспейсе не заняты (проверено по модулям и
именам крейтов).

### Направление зависимостей

- `ui` видит `logic` только через `Intent` и read-only срезы; мутирующих доступов нет.
- `logic` не знает про `ui` и `egui`; наружу отдаёт `Effect` и значения.
- `logic` вызывает `adapters` напрямую; `adapters` не знает ни про `logic`, ни про `ui`.
- `shell` — единственное место, где есть `&mut logic` и `egui::Context`.
- Общего `View`-снапшота нет: каждая страница получает те 2–4 среза, которые рисует. Её
  сигнатура и есть контракт.

### Разрешения для UI

Три правила, которые обязан соблюдать любой код в `ui/`:

1. **UI логики приложения** (screenshot, record, audio, library, convert, settings, region
   editor): рисует из `&`-срезов `logic`, изменения запрашивает через `Intent`. Настройки —
   единственное исключение, см. ниже.
2. **UI агента** (`ui/pages/agent`): это клиент агентских крейтов, а не слой логики приложения.
   Прямые чтения и вызовы `mediaa_agent`, `mediaa_engine`, `mediaa_queue` разрешены. `AgentUi`
   остаётся состоянием вида рядом с отрисовкой. Логика агента в `logic` не переносится и не
   оборачивается в `Intent`.
3. **Запрещено всей UI**: `mediaa_platform::*`, `rfd`, `std::fs`, `std::thread`, команды окну и
   viewport. Это содержание `adapters/`. Нарушения в `pages/agent` перечислены в `ARCH-14.6`.

### Настройки

`edit_settings`: UI получает дерево `Settings` через узкий доступ и меняет данные; последствия
(тема, хоткеи, preview-кэш, планирование записи на диск) применяет `logic` сам. `settings_changed`
исчезает как ручной вызов: забыть его невозможно, потому что изменение помечает сам доступ.
Исключение ограничено типом `Settings`; расширять его на другое состояние нельзя.

### Что где живёт

- **UI-состояние**: hover, focus, scroll, размеры, открытые дропдауны и вкладки, драфты ввода,
  выделение в списке, `AgentUi`.
- **Логика**: всё, что влияет на фоновую работу, сохраняется, шарится между треем/хоткеем/окном
  или является terminal result: сессия записи, job'ы захвата, индекс библиотеки, target записи,
  модель редактора области, очередь уведомлений, видимость окна.
- Критерий: если состояние переживает закрытие панели или меняет мир вне окна — это логика.

### Карта переезда

| Сейчас | Станет |
|---|---|
| `app.rs` (`App`, `logic()`, `ui()`, `on_exit`) | `shell.rs` |
| `app/state.rs` (`Page`, `Pending`), `app/command.rs` | `logic/navigation`, `logic/intent` |
| `app/actions.rs` (трей, хоткеи, оверлей) | `adapters/*` + `logic` через `Intent` |
| `app/capture.rs`, `app/recording.rs` | `logic/capture`, `logic/recording` |
| `app/window.rs`, флаги видимости | `logic/window_presence` + `Effect` |
| `app/notices.rs` (модель + вид) | `logic/notices` + `ui/notices` |
| `app/desktop.rs`, `app/recovery.rs` | `adapters/desktop`, `logic/recovery` |
| `features/recording` | `logic/recording` |
| `features/capture` (без `Overlay`) | `logic/capture` |
| `features/conversion` (без `ui()`, `controls.rs`) | `logic/conversion` + `ui/pages/convert` |
| `features/library` (без `ui()`, `grid`, `viewer`, `hover_preview`, `thumbs`) | `logic/library` + `ui/pages/library` |
| `features/agent` (198 строк) | `logic/agent_host.rs` |
| `features/cache_maintenance` | `logic/cache_maintenance.rs` |
| `pages/*` | `ui/pages/*` |
| `pages/agent/*` | `ui/pages/agent/*` (без переноса бизнеса) |
| `pages::header` | `ui/widgets` |
| `overlay.rs` + `overlay/editor.rs` | `logic/region_editor` + `ui/region_editor` |
| `services/clipboard`, `services/settings_writer` | `adapters/clipboard`, `adapters/settings_file` |
| `tray.rs`, `hotkeys.rs`, `region_marker.rs` | `adapters/tray`, `adapters/hotkeys`, `ui/region_marker` |
| `jobs.rs`, `latest_worker.rs` | `logic/jobs.rs`, `logic/latest_worker.rs` |
| `widgets/`, `themes/`, `icon.rs`, `i18n.rs` | `ui/` без изменений |

## Этапы

Каждый этап заканчивается зелёными `cargo check --workspace --lib --bins --profile dev-check`,
применимыми тестами, `cargo clippy --workspace --all-targets` и `cargo fmt --all -- --check`.
Этапы `ARCH-14.2` и `ARCH-14.3` допустимо выполнить одной партией: они трогают одни файлы.
Коммит создаётся только по разрешению пользователя на конкретную партию.

| Этап | Работа | Результат | Оценка |
|---|---|---|---|
| `ARCH-14.1` | Правила в `architecture.md`, проверка границ одной командой | граница зафиксирована, код не тронут | 1 сессия |
| `ARCH-14.2` | `Intent` на `about`, `audio`, `capture`; снять `Deref`; `AgentServices` → `logic/agent_host.rs` | цикл `features → pages` разорван | 2–3 сессии |
| `ARCH-14.3` | `WindowPresence`; чистые решения записи и снимка в `logic/` | один владелец окна; планирование записи и снимка без `egui` | 3 сессии |
| `ARCH-14.4` | `overlay` → `logic/region_editor` + `ui/region_editor` + `adapters` для диалога и окна | `overlay` исчезает как отдельная сущность | 2 сессии |
| `ARCH-14.5` | `notices` (очередь/вид), `edit_settings`, вынос `ui()`, `grid`, `viewer`, `thumbs` из контроллеров, `pages/` → `ui/pages/` вместе с `header` | `logic` без рендеринга, ручной `settings_changed` удалён | 2–3 сессии |
| `ARCH-14.6` | Эффекты из `ui/pages/agent` → `adapters` (`rfd`, reveal, чтение лога, звук, буфер) | UI агента соблюдает правило 3 | 1 сессия |

Вынос `mediaa-ui` в отдельный крейт — не этап, а отдельное решение после `ARCH-14.6`. Если
границу начнут нарушать, выносится UI (`crates/ui` → `mediaa-ui`), логика остаётся в `mediaa`.

## Проверка

После каждого этапа:

```text
cargo check --workspace --lib --bins --profile dev-check
cargo test -p mediaa --lib layers_have_no_forbidden_dependencies
cargo test -p mediaa --lib
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
```

Границы проверяет тест `crates/app/src/layer_check.rs`: он сканирует слои и падает на новом
запрещённом импорте. Временные исключения перечислены там же и названы этапом, который их
удаляет; устаревшее исключение тоже валит тест.

Плюс поведенческая проверка: `ui_tests` interaction/geometry, responsive, light/Russian/DPI и
snapshot-тесты остаются без изменения сценариев. Для этапа 3 обязательны тесты возврата окна
(`UI-3`, `TRAY-4`, `TRAY-5`, `UI-13`), для этапа 4 — overlay-тесты `Tab`, переключения монитора
и сохранения.

## Ход работ

Обновляется по мере этапов; это единственное место, где живёт статус.

- `ARCH-14.1` — готово. Правило в `architecture.md` 7.8, тест границ, исключения с этапами.
- `ARCH-14.2` — готово. `logic/` (`intent.rs`, `agent_host.rs`, `jobs.rs`, `latest_worker.rs`),
  `ui/` (`caches.rs`, `widgets/`, `themes/`); `AgentHost` без `Deref` и без `egui`; `AgentServices`
  переехал из `pages/agent` в `logic/agent_host`; `about`, `audio` и `capture` возвращают
  `Vec<Intent>` и не принимают `&mut App`; `about` больше не владеет кэшем логотипа — он в
  `ui/caches.rs`. `header` уехал в `ui/widgets` не здесь, а в `ARCH-14.5` вместе с `pages/`.
- `ARCH-14.3` — готово. `logic/window_presence.rs` заменил три флага (`hidden_by_us`, `ghosted`,
  `recording_quiet`) одним владельцем с явными переходами `hide`/`show`/`take_ghost`/`set_quiet`;
  `quit_when_idle` остался в `App`, потому что это жизненный цикл выхода, а не видимость.
  `logic/recording.rs::plan` собирает `RecordingRequest` из настроек и каталогов;
  `logic/screenshot.rs::plan` решает, что делать со снимком (файл, буфер, формат, путь). Оба модуля
  без `egui` и покрыты unit-тестами. Оркестрация эффектов (`show_window`, `Overlay`, уведомления,
  `Pending`) остаётся в shell: это композиция эффектов, а не решение. Поэтому `app/capture.rs` и
  `app/recording.rs` переезжают в `logic/` не целиком — из них уходят чистые части, и этап закрыт
  по владению, а не по расположению файлов.
- `ARCH-14.4` — готово. `overlay.rs` исчез: модель и решения в `logic/region_editor.rs` (`Purpose`,
  `Outcome`, `SaveDefaults`, `resolve_save_path`, `save_formats`, `save_stem`, `save_crop` — кроп,
  аннотации и запись файла), окно и ввод в `ui/region_editor.rs` + `ui/region_editor/editor.rs`.
  `rfd` ушёл в `adapters/dialogs`, отключение анимации открытия и исключение из захвата — в
  `adapters/native_window`. Добавлены 3 unit-теста модели и перенесён тест расширения. Взаимодействие
  (перетаскивание, рамка текста, hit-test) осталось в UI намеренно: это обработка ввода, а не
  решение. Взамен в тест границ добавлено исключение для `features/capture`: контроллер держит
  `Overlay` до `ARCH-14.5`.
- `ARCH-14.5` — в работе. Сделано: весь каталог страниц переехал в `ui/pages/` (19 512 строк, 26
  файлов), `crate::pages` больше не существует; `header` живёт там же, где страницы, и отдельный
  перенос в `ui/widgets` не понадобился. Системные эффекты страницы настроек (выбор папки и
  создание каталога) ушли в `adapters/dialogs::pick_folder` и `adapters/files::ensure_dir`, так что
  `ui/pages` не содержит `rfd` и `std::fs`. `notices` разделён: очередь, уровни, сроки жизни и
  `hover pause` — `logic/notices.rs`; карточки и анимация — `ui/notices.rs`; `app/notices.rs`
  оставляет только `App::error/warning/info/success`. Тест границ ловит `crate::ui` в `features` и
  эффекты в `ui`; исключения для агентских страниц (`rfd`, reveal, чтение лога) названы
  `ARCH-14.6`. Сделано также: отрисовка конвертации вынесена из контроллера —
  `ui/pages/convert/controller.rs` (весь `impl` с `ui`/`ui_inner`) и `ui/pages/convert/controls.rs`;
  `features/conversion` больше не импортирует `crate::ui`, а выбор файлов и папки идёт через
  `adapters/dialogs`. Ещё сделано: отрисовка просмотрщика вынесена из `features/library/viewer.rs`
  в `ui/pages/library/viewer.rs` — `impl Viewer` с `show`, областью медиа, top bar, controls и
  таймлайном; состояние, открытие файла, jobs загрузки и `Drop` остались в features. Попытка вынести
  `LibraryController::ui` и `actions.rs` (модалки удаления, переименования и очистки) упирается в
  то, что рисование размазано по трём файлам и завязано на приватные поля контроллера; библиотека
  вынесена отдельным заходом. Сделано в следующем проходе: библиотека разделена целиком —
  `features/library/mod.rs` сократился с 1169 до 649 строк (состояние, индекс, watcher, hits,
  удаление, cleanup), рисование ушло в `ui/pages/library/` (`controller.rs` 528, `actions.rs` 465,
  `grid.rs` 377, `viewer.rs` 521). `Overlay` больше не принадлежит `CaptureController`: им владеет
  shell (`App.overlay`), поэтому `logic/capture` не импортирует `crate::ui`. `edit_settings`
  сделано в форме, которая не создаёт лишних записей: `App::edit_settings` применяет дискретное
  изменение и сразу помечает настройки dirty; сайты, где страница держит `&mut Settings` для
  непрерывного редактирования (слайдеры, чекбоксы), оставлены как есть — они сигналят
  `Intent::SettingsEdited` по факту `changed()`. Guard на `DerefMut` сознательно не сделан: он
  помечал бы dirty каждый кадр и писал бы файл настроек бесконечно.

### Финальная раскладка

Сделано после закрытия этапов (механика, без изменения поведения):

- `features/` больше нет: контроллеры переехали в `logic/` (`capture`, `conversion`, `library`,
  `recording/controller.rs`, `cache_maintenance`), рядом лежат `logic/recording/plan.rs`,
  `logic/notices.rs`, `logic/region_editor.rs`, `logic/intent.rs`, `logic/window_presence.rs`,
  `logic/screenshot.rs`.
- `app.rs` и `app/` стали `shell.rs` и `shell/`; `shell/state.rs` оставил каталоги и desktop-события,
  а `Page` и `Pending` переехали в `logic/navigation.rs` — навигация это логика, и это убрало
  последнюю зависимость `logic → shell`.
- `services/` больше нет: `adapters/clipboard_worker.rs` и `adapters/settings_file.rs`.
- `region_marker.rs` и `tray.rs` переехали в `ui/region_marker.rs` и `adapters/tray.rs`.
- Перерисовка вместо `egui::Context`: корневой `wake::Wake`. Его принимают `adapters` (clipboard,
  tray) и `logic` (conversion, library poll, jobs), а `shell` единственный умеет превратить его в
  `ctx.request_repaint*`.
- Исключений в тесте границ больше нет: `TextureHandle` и воркеры декодирования разъехались
  (`ARCH-15.1`), остался только `hotkeys.rs` в корне (`ARCH-15.2`).
- `ARCH-14.6` — готово. Все эффекты агентских экранов ушли в `adapters`: выбор файлов каталога
  (`dialogs::pick_catalog_files`), reveal (`desktop::reveal`), чтение лога программы
  (`log_file::read_program_log`), копирование текста (`clipboard::copy_text`), выход звука
  (`audio::audio_out`). `ui/pages/agent` больше не содержит `rfd`, `std::fs` и платформенных
  модулей эффектов; исключения в тесте границ удалены. Попутно появились `adapters/desktop`,
  `adapters/files`, `adapters/log_file`, `adapters/clipboard`, `adapters/audio` — это и есть слой
  эффектов, описанный в `architecture.md` 7.4.

Текущая раскладка на момент последнего обновления: `app/`, `adapters/` (`audio`, `clipboard`,
`desktop`, `dialogs`, `files`, `log_file`, `native_window`), `features/`, `logic/`, `services/`,
`ui/` (`ui/pages`, `ui/region_editor`, `ui/notices.rs`, `ui/widgets`, `ui/themes`, `ui/caches.rs`)
плюс корневые `icon.rs`, `i18n.rs`, `region_marker.rs`, `tray.rs`, `hotkeys.rs`. Тест границ действует:
`features` не может импортировать `crate::ui` без исключения, `logic` не может использовать `egui`,
`ui` не может звать ОС и файлы,
`adapters` не может импортировать `ui` и `logic`.

## Материальные риски

- **Точка входа UI-тестов.** 120 тестов и 74 снимка строятся на `App`. При смене точки входа
  часть падений будет ложной. Правило: сценарии не меняются, снимки не обновляются, кроме случая
  реального изменения вёрстки (такого быть не должно).
- **Видимость окна.** 4 флага кодируют правила трея, хоткея и UI одновременно. Ошибка = окно не
  вернулось после оверлея. Сводить флаги можно только вместе с прогоном соответствующих тестов.
- **Terminal result записи.** При переносе `app/recording.rs` нельзя терять finalizer: `Saving`
  удерживает его до terminal event, иначе пользовательский файл теряется.
- **`overlay`.** 16 тестов, часть со snapshot'ами; `Tab` и переключение монитора легко сломать.
  Модель редактора переносится без изменения геометрии, вид — без изменения отрисовки.
- **Исключение для агента.** Без явной записи в `architecture.md` следующая модель «исправит»
  агент по общему правилу «UI не вызывает логику» и утащит 15 887 строк за прослойку. Правило
  записано в разделе 7.2.6 архитектуры и в этом документе.
- **`edit_settings`.** Единственная точка, где UI пишет данные. Расширение на другое состояние
  вернёт прямую мутацию логики из UI.
- **Один крейт.** Граница держится видимостью и проверками, а не компилятором. Проверки обязаны
  выполняться в каждом этапе, иначе через несколько задач граница снова станет условной.

## Что не делать

- Общий event bus, DI-контейнер, universal repository.
- Клонирование общего снапшота каждый кадр и центральный `View` вместо `App` — это новый god-объект.
- Полный MVU с отложенным на кадр откликом для слайдеров и перетаскивания.
- Отдельный крейт на функцию; переименование ради переименования.
- Перенос логики агента в `crates/app` и оборачивание агентских вызовов в `Intent`.
- Перенос `t!()` из логики в типизированные ключи: тексты уведомлений — решение логики, перевод
  строк — отдельная задача.

## Обновление документации

`architecture.md` описывает текущее состояние: целевое дерево переносится туда по мере
реализации — `ARCH-14.2` добавляет `logic`/`ui`, `ARCH-14.4` — region editor, `ARCH-14.5` —
notices и настройки.

Правило направления зависимостей — не описание состояния, а согласованная политика, поэтому
`ARCH-14.1` записывает его сразу.

До реализации в `architecture.md` добавлены: факт смешанности слоёв со ссылкой на этот документ и
правило, что `pages/agent` — клиент агентских крейтов.
### Тесты и слои

`ARCH-16`. UI-регресс тесты больше не живут в логике: тесты `tools`/`tools_strip`, `cleanup_modal`,
`grid_columns`/`cover_uv`, `Thumbnails` и снапшоты просмотрщика переехали в `ui/pages/library/`.
Тест границ для слоя `logic` теперь сканирует и `#[cfg(test)]`-код, и файлы `tests.rs`, поэтому
новый импорт `ui`/`egui` в тестах логики валит проверку. Логика тестов не менялась — изменилось
только место и импорты.

### Аудит тестов по слоям

`ARCH-17`. Проверены все тесты `ui`, `shell`, `adapters` по критериям: что именно проверяется,
какие импорты, есть ли реальные эффекты ОС, соответствует ли файл предмету теста.

- `shell/tests.rs` разгружен: три теста отрисовки уведомлений ушли в `ui/notices.rs`, два теста
  очереди и текста — в `logic/notices.rs`. В `shell` остался `notice_copy_action_...`, потому что
  он про `App`.
- Общий мьютекс harness-тестов и `LocaleGuard` переехали из `shell/test_support.rs` в корневой
  `src/test_support.rs` (только тестовая сборка): `ui` больше не ссылается на `shell` даже в тестах.
- Локальные дубли `LocaleGuard`/`english_locale`/`russian_locale` в `ui/region_editor.rs` удалены.
- `adapters` (9 тестов) и `shell` (134) проверяют то, что им положено: эффекты и композицию.

### Горячие клавиши

`ARCH-15.2`. Корневой `hotkeys.rs` удалён. Регистрация (`GlobalHotKeyManager`, событие, `Wake`) —
`adapters/hotkeys.rs`; правила слотов и группировка (`HotkeySlot`, `conflicting_slot`,
`grouped_actions`, `ordered_actions`) — `logic/hotkeys.rs`; захват комбинации из `egui::Key` —
`ui/pages/settings/hotkeys.rs`. Тип `HotkeyAction` переехал в `mediaa_core::settings`, чтобы адаптер
не зависел от `logic`.

### Страницы и Ctx

`ARCH-18`. `ui` больше не принимает `&mut App`. Страницы получают `ui/pages/ctx.rs::Ctx` — набор
срезов состояния на кадр; изменения применяют к срезам, эффекты возвращают через `Vec<Intent>` или
`AppCommand`. `ui::pages::shell` рисует рамку, уведомления и страницу и возвращает действие
уведомления; `App::frame_ui` собирает `Ctx`, выполняет интенты и копирует навигацию обратно.
`ui::pages::library::{apply_action, poll_cleanup}` обёрнуты в `App::apply_library_action` и
`App::poll_library_cleanup`. Тест границ теперь запрещает `crate::shell` в `ui`.
