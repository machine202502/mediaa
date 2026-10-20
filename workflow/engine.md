# Движок инференса отдельно от приложения — проект

Проект задачи «Движок инференса отдельно от приложения (E1)» и «Скачивание моделей — в движке (E2)»
из [todos.md](todos.md) (раздел 4). Справочник по работе — [dev-context.md](dev-context.md);
что сделано — [done.md](done.md). Этот файл — контракт для всех подзадач: код пишется по нему,
расхождения сначала правятся здесь.

## 0. Итоговая архитектура (решение пользователя от 2026-10-21, реализовано в E-8)

Пользователь не должен запускать вторую программу, и адрес движка никто настраивать не будет.
Цель выноса — чтобы код инференса жил отдельно от кода приложения (отдельный крейт), а не отдельная
программа. Поэтому:

* **Движок работает внутри приложения, вызовы — прямые.** `crates/engine` — библиотека, которую
  `mediaa` линкует. Её API — трейт `Engine` (бывший `Backend` HTTP-сервера) и его реализация над
  библиотекой: чат со стримингом через колбэк, эмбеддинги, реранк, сессия VAD как объект,
  распознавание, диаризация, модели (список, осмотр, скачивание с прогрессом, удаление, проверка),
  состояние, бюджет. Агент и приложение держат `Arc<dyn Engine>` и вызывают его из своих рабочих
  потоков. **HTTP нет** (ни сервера, ни клиента, ни порта): внутри одного процесса он только
  сериализует данные (PCM в base64) и открывает порт для любой программы на компьютере.
* **Отдельной программы `mediaa-engine` нет** (бинарник, `main.rs`, clap, `config.toml` движка,
  токен — удаляются). Снова один exe; отложенная загрузка Vulkan — снова в `crates/app/build.rs`.
* **Нет настроек адреса и токена** (`EngineSettings`, запись в хранилище паролей, поля в интерфейсе —
  удаляются). Состояние движка в интерфейсе: «запускается» / «работает» / «ошибка».
* **Модели** — в папке данных приложения, одной на компьютер: `paths::data_dir()/models`
  (раскладка `<owner>--<repo>` прежняя). Бюджет памяти — из настроек приложения (страница «Бюджет»),
  передаётся движку при старте и при изменении.
* Раздел 3 (HTTP API) больше не действует как транспорт; его смысловой состав (какие операции,
  какие поля) — это методы и типы трейта `Engine`.

**Реализовано (E-8):** трейт `Engine` и `LocalEngine::new(models_dir, budget)` — в `crates/engine`;
HTTP-сервер, программа `mediaa-engine`, её конфиг и токен, HTTP-клиент `crates/agent/src/engine.rs` удалены;
приложение создаёт один `LocalEngine` на `paths::models_dir()` и делит его как `Arc<dyn Engine>`; в
интерфейсе — «Движок работает» / «Движок не запустился: …». Тесты: `mediaa_engine::testing::FakeEngine`
(feature `testing`), живые и бенч-тесты — `MEDIAA_TEST_MODELS`.

**Разделы 1–5 ниже — история первого варианта (отдельная программа, HTTP).** Они не действуют;
сохранены, чтобы было видно, откуда взялись решения и состав операций трейта `Engine`.

## 1. Цель и решения (первоначальный вариант)

**Цель.** Приложение MediaA не содержит движка: ни llama.cpp/Vulkan, ни sherpa-onnx, ни скачивания
моделей. Всё это — отдельная программа `mediaa-engine`, которую пользователь ставит и запускает сам.
Приложение — тонкий клиент по HTTP.

**Решения:**

1. **Отдельный крейт `crates/engine`** (библиотека `mediaa_engine` + бинарник `mediaa-engine`) в том же
   workspace. Приложение (`crates/app`) и крейт агента (`crates/agent`) на него **не зависят** после
   переезда: только HTTP.
2. **В движок уходит весь вывод моделей:** языковые модели, эмбеддинги, реранкер (llama.cpp) и речь —
   VAD, распознавание, диаризация (sherpa-onnx). Иначе приложение по-прежнему тащит движок.
3. **Модели хранит движок** в своей папке, а не в `.ai/models` библиотеки: по умолчанию
   `%LOCALAPPDATA%\MediaA-engine\data\models` (конфиг — `%APPDATA%\MediaA-engine\config\config.toml`,
   оба через `directories::ProjectDirs::from("dev", "MediaA", "MediaA-engine")`); папка задаётся в
   конфиге или флагом `--models-dir`. Раскладка: `<models_dir>/<owner>--<repo>[--<file>]` — одна
   на весь движок (старая `ai_dir/models` удаляется в E-4). Скачивание, осмотр репозитория до
   скачивания, удаление — API движка.
4. **Бюджет памяти моделей и их загрузку/выгрузку ведёт движок** (нынешний `runtime` + пулы
   GPU/RAM планировщика). В приложении остаются только **слоты по видам работ** (сколько задач
   одновременно) и очередь задач.
5. **API совместим с llama-server (OpenAI) там, где можно:** чат со стримингом, `json_schema`,
   DRY-параметры с именами llama-server, `/v1/embeddings`, `/v1/rerank`. Тогда приложение без
   переделок работает и с `llama-server`/совместимыми серверами для языковых моделей. Своё —
   под `/mediaa/v1/…`: речь, модели, состояние.
6. **Связь:** HTTP на `127.0.0.1:47811` по умолчанию (порт и адрес — в конфиге движка и в настройках
   приложения). Без авторизации на localhost; опциональный токен `Authorization: Bearer` — если
   движок слушает не только localhost.
7. **Приложение движок не запускает.** При недоступности — понятный статус «Движок не найден: запустите
   mediaa-engine или укажите адрес» в шапке раздела «Агент» и на страницах моделей/бюджета/очереди.
8. **Звук в движок передаёт приложение** как PCM: 16 кГц, моно, f32 little-endian (как сейчас
   `SAMPLE_RATE`). Декодирование (FFmpeg) остаётся в приложении — движку FFmpeg не нужен.
9. Без обратной совместимости (CLAUDE.md): старые `.ai/models` библиотеки больше не используются,
   настройки моделей просто указывают модели движка.

## 2. Как устроено сейчас (что переезжает)

| Модуль `crates/agent/src` | Что делает | Куда |
|---|---|---|
| `llm.rs` | загрузка GGUF, чат, грамматика JSON, DRY, `gpus()`, оценка памяти | движок |
| `embed.rs` | эмбеддинги и реранкер (llama.cpp) | движок |
| `speech.rs` | VAD, распознавание, диаризация (sherpa-onnx), `words()` | движок (кроме чистых функций разбора, если нужны клиенту) |
| `runtime.rs` | загрузка/выгрузка моделей под бюджет, `with_model` | движок |
| `scheduler.rs` | слоты по видам работ + пулы памяти GPU/RAM | пулы → движок; слоты — `crates/agent/src/slots.rs` |
| `hub.rs`, `local.rs` | скачивание с Hugging Face, проверка файлов | движок |
| `check.rs` | проверка моделей по ролям | локальные модели — API движка; провайдеры — в агенте |

Вызовы, которые станут HTTP: `assistant/mod.rs` (`with_brain`), `assistant/brain.rs` (`LocalBrain`),
`assistant/scan.rs` (`LocalExtract`), `outline.rs`, `index.rs`, `search.rs`, `transcribe.rs`, `check.rs`;
в приложении — `pages/agent/{mod,models,budget,queue}.rs`.

## 3. API движка

Все ответы — JSON (кроме стримов SSE). Ошибки: HTTP-код + `{"error": {"message": "...", "kind": "..."}}`,
`kind` ∈ `not_found`, `bad_request`, `unauthorized` (401), `model_unsupported`, `out_of_memory`, `cancelled`, `internal`.
**Ошибка посреди SSE-потока** (чат, диаризация, скачивание) приходит событием `data: {"error":{"message","kind"}}`
(в чате) или `event: error` + тот же JSON (в остальных потоках); после неё поток закрывается без `[DONE]`.
Клиент обязан её разобрать и вернуть как ошибку.

### 3.1 Общее

* `GET /health` → `{"name":"mediaa-engine","version":"…","gpus":[{"name","backend","memory_total","memory_free"}],"models_dir":"…"}`
* `GET /mediaa/v1/status` → `{"loaded":[{"model","pool":"gpu|cpu","bytes","users"}],"waiting":[{"id","model","reason"}],"budget":{"gpu_bytes","ram_bytes"},"used":{"gpu_bytes","ram_bytes"}}`
* `PUT /mediaa/v1/budget` `{"gpu_bytes","ram_bytes"}` → `204`. Движок сохраняет в свой конфиг.

### 3.2 Модель в запросе

Во всех запросах вывода поле `model` — **спецификация модели**, как в настройках сейчас:
`owner/repo` или `owner/repo/path/file` (Hugging Face) или абсолютный путь к локальному файлу/папке.
Если модель не скачана — движок скачивает её сам (как сейчас `hub::download`), прогресс виден в
`/mediaa/v1/status` (`waiting.reason = "downloading"`, `waiting.progress = {"done": байты, "total": байты}`). Доп. поля запроса (расширения):
`"mediaa": {"n_ctx": 8192, "device": "auto|gpu|cpu", "priority": "interactive|background"}`.
Заголовок `X-HF-Token` — токен Hugging Face приложения (из системного хранилища).

### 3.3 Языковые модели (совместимо с llama-server)

`POST /v1/chat/completions`:
```json
{"model":"unsloth/Qwen3.5-4B-GGUF","messages":[{"role":"system","content":"…"},{"role":"user","content":"…"}],
 "max_tokens":2048,"temperature":0,"stream":true,
 "response_format":{"type":"json_schema","json_schema":{"schema":{…}}},
 "dry_multiplier":0.8,"dry_base":1.75,"dry_allowed_length":2,"dry_sequence_breakers":["\""],
 "mediaa":{"n_ctx":8192,"device":"auto","priority":"interactive"}}
```
* Нестрим: обычный ответ OpenAI (`choices[0].message.content`, `usage`).
* Стрим: SSE `data: {"choices":[{"delta":{"content":"…"}}]}` … `data: [DONE]`; последний чанк — с `usage` (`prompt_tokens`, `completion_tokens`); то же поле `usage` у нестрима.
* **Отмена:** клиент закрывает соединение → движок прекращает генерацию (проверка при каждой записи в
  поток) и освобождает модель.
* Значения DRY — те, что сейчас при `no_repeats: true` в `llm.rs` (взять оттуда точные числа).
* Рассуждения (`<think>`) — как сейчас `strip_reasoning`: в `content` не попадают.

`POST /v1/embeddings` `{"model","input":["…"],"mediaa":{"kind":"query|passage","n_ctx":2048,"device":"auto"}}` →
`{"data":[{"embedding":[…],"index":0}]}` (префиксы query/passage — как сейчас `embed::prompts`).

`POST /v1/rerank` `{"model","query","documents":["…"]}` → `{"results":[{"index":0,"relevance_score":0.93}]}`
(формат llama-server).

### 3.4 Речь (`/mediaa/v1/speech/…`)

Звук — тело запроса `application/octet-stream`, PCM f32 LE, 16 кГц, моно.

* **VAD как сессия** (состояние между кусками, как сейчас `Vad::push/finish`):
  * `POST /mediaa/v1/speech/vad` `{"model":"csukuangfj/vad/silero_vad.onnx","offset":123.4}` → `{"session":"…"}`
  * `POST /mediaa/v1/speech/vad/{session}/push` (PCM) → `{"segments":[{"start":…, "samples_b64":"…"}]}` —
    отрезки речи целиком (`samples` — base64 PCM f32 LE), как сейчас `Speech { start, samples }`.
  * `POST /mediaa/v1/speech/vad/{session}/finish` → то же + сессия закрывается.
  * Сессия без запросов 10 минут удаляется.
* `POST /mediaa/v1/speech/recognize?model=…&offset=…` (PCM отрезка) → `Phrase` как сейчас
  (`{"start","end","words":[{"text","start","end"}]}`).
* `POST /mediaa/v1/speech/diarize?segmentation=…&embedding=…&speakers=N` (PCM всей записи) → SSE:
  сейчас приходит только `event: result` `{"turns":[{"start","end","speaker"}]}` (у sherpa-onnx в
  Rust нет колбэка); `event: progress` `{"done","total"}` перед ним появится с задачей 1.2 в todos.

### 3.5 Модели (`/mediaa/v1/models…`, E2)

* `GET /mediaa/v1/models` → скачанные: `[{"spec","path","bytes","format":"gguf|sherpa-onnx|onnx|other","files":[{"name","bytes"}]}]`
* `POST /mediaa/v1/models/inspect` `{"spec"}` → **без скачивания** (HF API, список файлов и размеры):
  `{"spec","files":[{"name","bytes"}],"format","download_bytes","roles":["agent","embedding",…],"problems":["…"]}`.
  `roles` — для каких ролей подходит **по формату**; `problems` — почему нет (например «нет GGUF»).
  `download_bytes` — сколько скачает `pull` (для GGUF — один выбранный квант). Роли `speech`
  (синтез) пока нет ни у одной модели. Чтение архитектуры GGUF из заголовка (`Range`), чтобы
  отличать языковую модель от эмбеддинговой, — задача 4.1 в todos.
* `POST /mediaa/v1/models/pull` `{"spec"}` → SSE `event: progress {"done","total"}` … `event: done {"path"}`.
* `DELETE /mediaa/v1/models?spec=…` → `204`.
* `POST /mediaa/v1/models/check` `{"spec","role"}` → пробный запуск (как сейчас `check.rs` для локальных):
  `{"ok":true,"bytes":…}` или ошибка `model_unsupported`. Скачивает модель при необходимости и запускает её
  одним синхронным вызовом (без прогресса; для прогресса — сначала `pull`).
* Поле `spec` в списке и ответах — `owner/name` для репозитория (файл внутри — отдельно в `files`).

## 4. Сторона приложения

* Настройки (`core::agent::AgentSettings`): `engine: EngineSettings { url: String = "http://127.0.0.1:47811", token: String (keychain) }`.
  Источник модели `ModelSource::HuggingFace`/`Local` теперь значит «модель движка» (спецификация
  передаётся движку). `Provider` — как сейчас.
* Клиент `crates/agent/src/engine.rs`: блокирующий `reqwest`, методы по API выше, SSE-разбор (как в
  `providers.rs`), отмена через `AtomicBool` → закрыть ответ.
* `Brain`: `LocalBrain` → `EngineBrain` (чат движка с `json_schema` + DRY + стрим, разбор шага как сейчас,
  `loop_start` остаётся у клиента). `scan::LocalExtract`, `outline::digest` — через тот же чат.
  Приоритет: разговор в чате — `interactive`, скан и главы — `background`.
* `index.rs`/`search.rs` — `/v1/embeddings`, `/v1/rerank`. `transcribe.rs` — декодирует как сейчас и шлёт
  PCM в VAD-сессию, отрезки — в `recognize`, всю запись — в `diarize`. Возобновление по курсору — как сейчас.
  Модели VAD и сегментации — константы на стороне приложения (`transcribe.rs`), движок получает их в запросе.
* Слоты по видам работ — в агенте, модуль `crates/agent/src/slots.rs`; память и загрузка моделей — в движке.
* UI: статус движка (шапка раздела «Агент»), «Бюджет» — GPU из `/health`, память → `PUT /budget`,
  «Очередь» — модели в памяти из `/status`, «Модели» — скачанные/осмотр до скачивания/скачивание с
  прогрессом «Скачивание N из M ГБ»/проверка — через API движка. Проверка показывает «проверка» на весь
  вызов `check`; ожидающие в `/status` (очередь моделей, скачивания) — на странице «Очередь».

## 5. Порядок работ (подзадачи)

Каждый шаг оставляет приложение рабочим; тесты и clippy зелёные после каждого.

| # | Подзадача | Зависит | Исполнитель |
|---|---|---|---|
| E-1 | **Крейт `crates/engine` как библиотека:** перенести `llm, embed, speech, runtime, hub, local` и пулы/загрузку моделей; `check` для локальных моделей. Агент временно зависит от библиотеки движка (вызовы те же). Чистый перенос, без смены поведения | — | DeepSeek |
| E-2 | **HTTP-сервер `mediaa-engine`** (axum + tokio, тяжёлая работа в `spawn_blocking`): все эндпоинты раздела 3 поверх библиотеки; конфиг движка (адрес, порт, папка моделей, бюджет); тесты роутера на поддельном бэкенде (трейт) | E-1 | DeepSeek |
| E-3 | **Клиент `agent/src/engine.rs`** по разделу 3 + тесты на поддельном сервере (`tiny_http`) | контракт | DeepSeek (параллельно с E-2) |
| E-4 | **Переключить агент на клиент:** `EngineBrain`, scan, outline, index, search, transcribe, check; разделить планировщик (слоты остаются); настройки `engine`; убрать зависимость агента и приложения от библиотеки движка; в библиотеке оставить одну раскладку моделей (`*_in`-функции становятся единственными, старые `ai_dir/models` удаляются) | E-2, E-3 | DeepSeek / Sonnet |
| E-5 | **Интерфейс:** статус движка, «Бюджет», «Очередь», «Модели» через API (осмотр до скачивания, статусы, подсказки форматов по ролям); локали ru/en | E-4 | DeepSeek |
| E-6 | **Сборка и документы:** скрипты сборки двух программ, README, dev-context, todos/done | E-5 | Haiku |
| E-7 | **Проверка целиком (голова):** движок + приложение, распознать запись, задать вопрос агенту, бенч | все | голова |

Задачи из todos, которые после этого делаются уже на стороне движка: 4.1 (проверка до скачивания —
частично покрыта `inspect`), 4.2 (цепочки моделей), 3.7/3.9 (tool calling, thinking через
`--jinja`-подобные шаблоны движка), 1.1 (форматы распознавания).
