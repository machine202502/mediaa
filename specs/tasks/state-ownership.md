# ST-10: Кто владеет состоянием и кто его разбирает

## Короткий ответ

- **protocol** — типы, схема состояния и чистые правила переходов;
- **sync** — доставка, версии, порядок событий, resync;
- **backend** — единственный исполнитель переходов и владелец
  авторитетного состояния;
- **frontend** — проекция авторитетного состояния через те же чистые
  переходы + отдельное локальное UI-состояние.

Не «у каждого своя логика». Чистая логика переходов одна, а исполнение
разное: backend выполняет эффекты, frontend рисует проекцию.

## Почему не sync

`mediaa-sync` сейчас не знает домена и не должен. Если он начнёт разбирать
recording/tray/agent, он:

- потеряет транспортную независимость;
- потянет `mediaa-core/media/platform`;
- станет вторым backend;
- всё равно не сможет выполнять эффекты (запись, окно, файлы).

Sync должен уметь только:

```text
Hello -> Subscribe -> Snapshot/Changed
Event(seq) -> apply | ignore | gap -> Resync
Command(id) -> Result(id)
```

Это уже state machine, но **механизма**, а не домена.

## Почему не «каждый сам»

Если backend и frontend независимо пишут переходы:

- `Idle -> Starting` в одном, `Idle -> Recording` в другом;
- frontend оптимистично показывает `Saving`, backend уже `Failed`;
- tray open в backend `Opening`, во frontend `Hidden`;
- тесты проверяют разные таблицы.

Это и есть текущая проблема. Логика должна быть одна, а не «похожая».

## Слои

```text
protocol::state
  enum State
  enum Event
  enum Command
  fn apply(state, event) -> (state, Vec<Effect>)

backend
  owns authoritative State
  executes Effect -> IO
  publishes Snapshot/Event(revision)

sync
  delivers Snapshot/Event/Command/Result
  rejects stale revisions
  requests Resync on gap

frontend
  applies Snapshot/Event through protocol::state::apply
  renders projection
  sends Command
  owns frontend state, including persistent client preferences
```

`frontend` state is not necessarily ephemeral. Он может хранить своё
состояние на диске, но backend про него не знает и через sync его не гоняет.

## Матрица владения

| Состояние | Авторитет | Frontend |
|---|---|---|
| Tray/UI process | backend | только отображает и сообщает `WindowShown/Hidden` |
| Recording | backend | проекция `RecordingView`, локальные диалоги |
| Conversion job | backend | проекция batch, локальная форма |
| Library index | backend/storage | кэш отображения, выделение, скролл |
| Agent host | backend | проекция задач/стримов, вкладки |
| Settings persistence | backend | локальная форма до Save |
| Client preferences (volume, page, layout) | frontend | владеет и сохраняет на диск |
| Модалки, вкладки, hover, scroll | — | только frontend |
| Анимации, фокус, размеры | — | только frontend |

Правило: если состояние влияет на файлы, процессы, запись или других
клиентов — владелец backend. Если это только клиентский опыт — владелец
frontend, даже если он сохраняется на диск.

## Frontend-owned state

Frontend может и должен хранить у себя клиентские предпочтения:

- громкость просмотрщика;
- выбранная страница/вкладка;
- размеры и положение панелей;
- сортировка/фильтр библиотеки;
- последний открытый файл;
- тема, если она не влияет на backend;
- локальные флаги обучения/подсказок.

Это состояние:

- не является авторитетным доменным;
- не отправляется в backend;
- не проходит через `mediaa-sync`;
- хранится frontend-ом в отдельном файле (например `ui-state.toml`);
- имеет версию схемы и атомарную запись;
- тестируется без Windows: load/save/migrate/corrupt.

Пример: `viewer_volume` сейчас лежит в `mediaa_core::Settings` и
сохраняется backend-ом. Это неверно по новой модели: громкость нужна
только frontend-плееру. Нужна миграция: прочитать старое значение из
`Settings`, записать в `ClientPreferences`, удалить из backend-настроек
или оставить только как legacy-поле на один релиз.

Backend не должен знать про громкость, вкладки и последнюю страницу.
Единственное исключение — если значение влияет на запись, файлы или
поведение демона; тогда это доменное состояние backend.

## Где живёт общий reducer

Не в sync. Два варианта:

1. `mediaa-protocol::state` — протокол уже общий для backend/frontend и
   уже содержит доменные DTO. Чистые `State/Event/Effect/apply` лежат там.
2. Отдельный `mediaa-state` — чище, но ещё один крейт.

На первом этапе — `mediaa-protocol::state`: меньше телодвижений, оба крейта
уже зависят от protocol. Если protocol станет слишком тяжёлым, вынести в
`mediaa-state` без изменения API.

Reducer обязан быть:

- без `egui`, окон, потоков, файлов;
- время — параметр (`now`), не `Instant::now()` внутри;
- эффекты — enum, не прямой вызов IO;
- тесты — обычные unit-тесты, без Windows.

## Пример: tray

```rust
let (state, effects) = tray::apply(state, TrayEvent::ClickShow, now);
```

Backend:

```text
apply -> [SpawnVisible]
execute -> spawn process
publish Snapshot { state }
```

Frontend:

```text
receives Snapshot { state: Opening }
apply same reducer for UI projection
sends Command::Show
```

Frontend не решает «открыть/не открыть». Он показывает `Opening` и ждёт
`Open`/`Hidden` от backend.

## Пример: recording

Backend владеет `RecordingState`. Frontend получает `RecordingViewWire` и
применяет тот же `apply` для проекции. Кнопка Stop отправляет
`Command::Stop`; frontend не переводит себя в `Saving` до ответа backend,
кроме явного optimistic-состояния `Stopping`, которое исчезает при
`Failed`.

## Тесты

Один набор переходов в protocol:

```text
recording: start -> started -> stop -> saving -> finished
tray: show -> starting -> shown
conversion: probe -> ready -> running -> finished
```

Затем:

- backend: эффекты (spawn/save/quit) с фейковыми исполнителями;
- frontend: проекция (snapshot/event -> view) без egui;
- sync: версии/порядок/gap;
- integration: backend command -> sync -> frontend projection.

Не нужно писать две таблицы переходов. Нужно один раз описать переход и
дважды проверить его исполнение.

## Что запрещено

- backend и frontend с разными `match` на одно состояние;
- sync, знающий имена `tray_state`/`recording_start`;
- frontend, который сам придумывает `Visible`, пока backend `Opening`;
- хранение авторитетного состояния во frontend;
- «на всякий случай» обновлять и snapshot, и отдельный флаг.
