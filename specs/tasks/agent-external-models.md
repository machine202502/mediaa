# Агент без встроенного инференса: модели только через внешние подключения

Исполнение и статусы подзадач: [agent-external-steps.md](agent-external-steps.md).
Этот файл хранит решения. Раздел «Контекст: как устроено сейчас» описывает систему до
перевода на внешние подключения.

## Цель

MediaA не линкует и не хранит движки инференса. Из сборки уходят `llama.cpp`, Vulkan,
`sherpa-onnx`, загрузка весов с Hugging Face и бюджет GPU/RAM. Модель всегда описывается
подключением, то есть способом обратиться к тому, что пользователь установил сам: облачный API,
локальный сервер или CLI-команда. Приложение только вызывает такое подключение.

Подключения, которые нужно поддержать:

| Вид | Что это | Как вызываем |
|---|---|---|
| OpenRouter | облако, сотни моделей | HTTP, OpenAI-совместимый `/chat/completions`, `/embeddings`, `/models` |
| OpenAI | облако | то же; плюс `/audio/transcriptions` |
| DeepSeek | облако | OpenAI-совместимый `https://api.deepseek.com`; `reasoning_content` |
| Ollama | локально у пользователя | HTTP-API демона (`http://localhost:11434`); CLI `ollama` — только для проверки установки, списка, `pull` и запуска `serve` |
| OpenAI-совместимый сервер | LM Studio, `llama-server` (в т.ч. собранный пользователем с Vulkan), vLLM, LocalAI, speaches/faster-whisper-server, Groq, Gemini/Anthropic OpenAI-endpoints | HTTP по `base_url` |
| Hugging Face router | облако | HTTP, OpenAI-совместимый (уже есть) |
| Команда (CLI) | любая программа пользователя: `whisper-cli` (whisper.cpp), `sherpa-onnx-*` (ONNX), pyannote-скрипт и т.п. | запуск процесса по шаблону аргументов, разбор вывода в известном формате |

### Почему Ollama через HTTP, а не только через `ollama run`

Пользователь просил вызывать CLI `ollama`. `ollama run` не отдаёт вызовы инструментов,
JSON-schema ответа, usage-токены, эмбеддинги и не даёт надёжно отменить генерацию. Всё это агенту
нужно. Сам CLI только клиент того же демона на `localhost:11434`. Поэтому генерация и эмбеддинги
идут через HTTP-API демона (`/v1/chat/completions`, `/api/embed`, `/api/tags`). CLI используется
там, где он действительно нужен: проверить, что Ollama установлена (`ollama --version`), запустить
`ollama serve`, если демон не отвечает, и скачать модель (`ollama pull <name>`). Если пользователь
решит иначе, этот раздел меняется первым.

## Контекст: как устроено сейчас

Ветка `feature/agent`, перенос агента на архитектуру `main` (см. [agent-port.md](agent-port.md)).

- `crates/engine` (`mediaa-engine`) — локальный runtime. Зависит от `llama-cpp-2` (на Windows с
  `vulkan`), `sherpa-onnx`, `hf-hub`, `safetensors`, `memmap2`, `libloading`.
  - `engine/mod.rs`: трейт `Engine` и `LocalEngine`. Методы: `health`, `status`, `set_budget`,
    `set_wanted`, `forget_loaded`, `chat`, `chat_thinking`, `embeddings`, `rerank`, `vad_open`,
    `recognize`, `diarize`, `models`, `inspect`, `pull`, `delete_model`, `check_model`.
  - `llm.rs` (llama.cpp чат), `embed.rs` (эмбеддинги/реранк через llama.cpp), `speech.rs`
    (sherpa-onnx: VAD silero, ASR, диаризация pyannote+speaker embedding), `runtime.rs` +
    `scheduler.rs` (загрузка/выгрузка моделей в рамках бюджета памяти, keep-alive), `hub.rs` +
    `engine/downloads.rs` (скачивание с HF), `local.rs`, `engine/models.rs`, `engine/validate.rs`,
    `testing.rs` (`FakeEngine` под feature `testing`).
- `crates/core/src/agent.rs` — настройки: `ProviderKind {OpenAiCompatible, OpenRouter,
  HuggingFace}`, `Provider {id, name, kind, base_url, api_key(skip), proxy, context}`,
  `ModelSource {HuggingFace{repo}, Local{path}, Provider{provider, model}}`, `ModelRole` (7 ролей:
  Embedding, Reranker, Agent, Titles, SpeechRecognition, Diarization, Speech), `ModelChoices` с
  дефолтами на HF-репозитории GGUF/ONNX, `Device`/`ModelDevices`, `Budget` (gpu/ram/keep_alive +
  лимиты параллелизма по `WorkKind`), `AgentSettings`.
- `crates/agent`:
  - `providers.rs` — HTTP-клиент OpenAI-совместимых провайдеров (`chat_completion_stream`,
    список моделей, прокси, ошибки 401/403/429).
  - `assistant/provider.rs` — `ProviderBrain` (чат-цикл через HTTP с tools, reasoning).
  - `assistant/brain.rs` — `EngineBrain` (чат-цикл через `Engine::chat*`).
  - `assistant/mod.rs::with_brain`/`waiter` — ветвление `ModelSource::Provider` против
    локальной модели; `hf_token`.
  - `models.rs::local_spec` → `mediaa_engine::runtime::source_name`.
  - `transcribe.rs` — VAD (`engine.vad_open`) + `engine.recognize` по фразам, курсор `t`
    для возобновления, `VAD_MODEL`.
  - `diarize.rs` — диаризация чанками (`engine.diarize`), склейка спикеров между чанками по
    `Voice.embedding`; `diarize_quality.rs` — замеры качества.
  - `search.rs`, `index.rs` — `engine.embeddings`, `engine.rerank`.
  - `outline.rs`, `names.rs`, `assistant/scan.rs`, `title.rs`, `step_title.rs` — `engine.chat`.
  - `check.rs` — проверка выбранных моделей (`check_all`).
  - `scope.rs` — `Scope` прокидывает `Arc<dyn Engine>` и учитывает статистику вызовов.
  - `run.rs::Host { engine, slots, ... }`, `slots.rs` — лимиты параллелизма по `WorkKind`.
  - `live.rs` — тестовые прогоны на реальных моделях из `models_dir`.
- `crates/eval` — создаёт `LocalEngine` (`run.rs`), диаризация в `diarize.rs`/`score.rs`.
- `crates/app`:
  - `app.rs` создаёт `LocalEngine::new(paths::models_dir(), …)`.
  - `features/agent.rs::AgentController` — держит `Arc<dyn Engine>`, `engine_budget`,
    опрашивает `status` при открытом разделе.
  - `pages/agent/models.rs` — выбор источника по ролям, inspect/pull/delete HF-моделей,
    папка моделей; `providers.rs` — список провайдеров и проверка ключей; `budget.rs` —
    GPU/RAM/keep-alive и лимиты; `engine_state.rs` — загруженные модели/GPU.
- `core/src/paths.rs::models_dir`.
- Сборка: `.cargo/config.toml` (`LLAMA_STATIC_CRT`, `CMAKE_GENERATOR`, `CMAKE_TOOLCHAIN_FILE`),
  `cmake/static-crt.cmake`, `Directory.Build.targets`, проверки CMake/Vulkan в
  `scripts/windows/env.ps1`, README (CMake, Vulkan SDK), `specs/constraints.md` (Vulkan/CMake),
  профиль `dev` с `debug = 0` из-за D8050 на сборке llama.cpp
  ([windows-build.md](windows-build.md)).

## Ограничения

- Правила [rules.md](../rules.md): без обратной совместимости, миграций и fallback-путей;
  удалённые поля, locale-ключи, тесты и документация удаляются сразу. Комментарии в коде
  запрещены. Locale-ключи меняются синхронно в `en.toml` и `ru.toml`.
- Секреты только в OS keychain (`agent/secrets`), не в файле настроек.
- Новый крейт или trait только при реальной потребности. Отдельный HTTP-клиент на каждого
  провайдера не нужен: OpenRouter, OpenAI, DeepSeek, HF, Ollama `/v1` и прочие серверы — один
  OpenAI-совместимый клиент. Различия (URL по умолчанию, поле reasoning, эндпоинт эмбеддингов,
  список моделей) задаются данными пресета.
- Внешние процессы запускаются только через явный шаблон команды, который пользователь ввёл сам.
  Никакой оболочки (`sh -c`/`cmd /c`): исполняемый файл и аргументы отдельно, подстановка
  плейсхолдеров в аргументы. Отмена убивает процесс по известному child handle. Ведущий `~`
  в программе и в аргументах раскрывается в домашний каталог (`directories::UserDirs`); других
  раскрытий в стиле оболочки нет.
- Пользовательская инструкция по установке моделей — [docs/agent-models.md](../../docs/agent-models.md).
  Названия ролей, видов подключений, форматов вывода и аргументы скриптов в UI должны
  совпадать с ней. При расхождении правится и UI, и документ.
- Async runtime в проекте нет и не вводится. tokio сейчас приходит только транзитивно, через
  `reqwest` (включая `reqwest::blocking`) и `hf-hub` (`cargo tree -i tokio`). Своего async-кода
  нет. HTTP переводится на синхронный `ureq`, после чего tokio уходит из дерева зависимостей
  (см. «HTTP без tokio»).
- Потоки очереди (`agent-queue`, `queue-<kind>` в `agent/src/run.rs`) остаются. Блокирующие
  вызовы моделей идут в них, отмена — через `AtomicBool` и kill дочернего процесса. Поток
  `model-keepalive` из `LocalEngine` исчезает вместе с локальным runtime.
- Пользователь сам ставит Ollama, серверы и CLI. В приложении нет ни одного бинарника и ни одной
  модели, а в UI — подсказки, что установить.

## Целевая архитектура

### Понятия

- Подключение (`Connection`): сохраняемая запись в `AgentSettings.connections`. Поля: `id`,
  `name`, `kind`, параметры вида. Заменяет `Provider`.
- Вид подключения (`ConnectionKind`):
  - `OpenRouter`, `OpenAi`, `DeepSeek`, `HuggingFace` — фиксированный базовый URL из пресета,
    ключ в keychain;
  - `Ollama` — `base_url` (по умолчанию `http://localhost:11434`), путь к `ollama`
    (по умолчанию ищется в `PATH`), ключа нет;
  - `OpenAiCompatible` — произвольный `base_url`, ключ необязателен;
  - `Command` — исполняемый файл, шаблон аргументов, формат вывода (см. ниже).
  - Общие поля: `proxy`, `context` (как сейчас у `Provider`).
- Источник модели (`ModelSource`) — только `{ connection: String, model: String }`. Для
  `Command` поле `model` передаётся в шаблон как `{model}` и может быть пустым. Варианты
  `HuggingFace` и `Local` удаляются.
- Возможности (`Capability`): `Chat`, `Embeddings`, `Rerank`, `Transcription`, `Diarization`,
  `VoiceEmbedding`. Каждый вид подключения декларирует, что умеет:

| Вид | Chat | Embeddings | Rerank | Transcription | Diarization | VoiceEmbedding |
|---|---|---|---|---|---|---|
| OpenRouter | да | да (`/embeddings`) | нет | нет | нет | нет |
| OpenAI | да | да | нет | да (`/audio/transcriptions`) | нет | нет |
| DeepSeek | да | нет | нет | нет | нет | нет |
| HuggingFace router | да | нет | нет | нет | нет | нет |
| Ollama | да | да (`/api/embed`) | нет | нет | нет | нет |
| OpenAiCompatible | да | да | да (`/rerank`, формат Jina/Cohere — llama-server, vLLM, LocalAI) | да | нет | нет |
| Command | нет | нет | нет | да | да | да |

  Возможность команды определяется её форматом вывода: `Rttm` → `Diarization`, `Vectors` →
  `VoiceEmbedding`, остальные → `Transcription`.

  UI роли показывает только подключения с нужной возможностью. Если сервер всё же не поддержал
  возможность, это видно на проверке («Проверить»): ошибка эндпоинта показывается как есть.

### Роли

- `Agent`, `Titles` → `Chat`.
- `Embedding` → `Embeddings`; `Reranker` → `Rerank` (необязательна, как сейчас).
- `SpeechRecognition` → `Transcription`.
- `Diarization` → `Diarization` (необязательна; без неё `AnalysisOptions.diarize` выключается
  как сейчас в `resolved`).
- Новая роль `VoiceEmbedding` («Отпечаток голоса») → `VoiceEmbedding`. Необязательна. Без неё
  спикеры между кусками склеиваются только по перекрытию (см. «Диаризация»).
- `Speech` удалить: роль без источника по умолчанию и без потребителя. Сначала проверить через
  `grep -rn "ModelRole::Speech\b"`. Если потребитель найдётся, оставить и сопоставить с
  `Transcription`.
- Дефолтов на HF-репозитории больше нет: `ModelChoices::default()` — все `None`. Первый запуск
  ведёт пользователя в раздел подключений.

### Крейт `engine`

Крейт остаётся `mediaa-engine`, но становится слоем подключений, а не runtime. Переименование
не делать: оно раздует diff без пользы.

```
crates/engine/src/
  lib.rs            pub use engine::*;
  engine/mod.rs     trait Engine + struct Connections (реализация)
  engine/types.rs   ChatRequest, ChatAnswer, Usage, EmbeddingsRequest, RerankRequest,
                    RerankResult, TranscribeRequest, Diarization, ModelInfo, ModelCheck
  engine/error.rs   EngineError (без вариантов загрузки/памяти)
  openai.rs         OpenAI-совместимый клиент: chat (stream, tools, reasoning), embeddings,
                    rerank, audio/transcriptions, GET /models; прокси, ключ, коды ошибок
                    (перенос из agent/src/providers.rs)
  ollama.rs         /api/tags, /api/embed, `ollama --version`, `ollama serve`, `ollama pull`
  command.rs        запуск CLI по шаблону, разбор вывода (whisper JSON, SRT/VTT, RTTM, векторы)
  presets.rs        базовые URL и особенности видов (поле reasoning, есть ли /embeddings)
  testing.rs        FakeEngine (feature `testing`)
```

Удаляются целиком: `llm.rs`, `embed.rs`, `speech.rs` (после переноса склейки голосов в
`agent/src/voices.rs`), `runtime.rs`, `scheduler.rs`,
`local.rs`, `hub.rs`, `engine/downloads.rs`, `engine/models.rs`, `engine/validate.rs`.
Зависимости остаются `mediaa-core`, `anyhow`, `log`, `serde`, `serde_json`, добавляется
`ureq`; удаляются `reqwest`, `llama-cpp-2` (обе записи), `sherpa-onnx`, `hf-hub`, `safetensors`,
`memmap2`, `libloading`.

### HTTP без tokio

- Единственный HTTP-клиент — `ureq` 3 (синхронный, без async runtime) с features `json`,
  `socks-proxy` и TLS через rustls. Вынести его в `[workspace.dependencies]` и подключить в
  `engine` и `agent`. `reqwest` удалить из `engine/Cargo.toml` и `agent/Cargo.toml`.
- Потребители `reqwest` сейчас: `agent/src/providers.rs` (переезжает в `engine/openai.rs`),
  `agent/src/web.rs` (DuckDuckGo, SearXNG, Brave, Tavily: `.query(...)`, прокси),
  `agent/src/transcribe.rs`. Найти все: `grep -rn "reqwest" crates --include=*.rs`.
- Один `ureq::Agent` на подключение хранится в `Connections` и пересоздаётся только при
  изменении прокси или таймаута. Сейчас `providers.rs::client` строит новый клиент на каждый
  запрос, и каждый `reqwest::blocking::Client` поднимает собственный поток с tokio. Для `web.rs` —
  один `Agent` на `WebSettings`.
- Соответствие возможностей:
  - таймауты — `timeout_global` (чат 300 с, остальное 20 с, как сейчас в `providers.rs`);
  - SSE — построчное чтение `body_mut().as_reader()` через `BufReader::lines()`, проверка
    `cancel` и `pulse::beat()` между строками, как сейчас;
  - прокси `http://`, `https://`, `socks5://` — `ureq::Proxy::new`; ошибка разбора остаётся
    текстом `proxy "<url>": …`;
  - коды ответа: выключить «ошибку на 4xx/5xx» (`http_status_as_error(false)`) и разбирать
    статус самим, сохранив тексты 401/403/429 из `providers.rs`;
  - `multipart/form-data` для `/audio/transcriptions`: в `ureq` 3 его нет. Перед реализацией
    проверить, есть ли поддерживаемый crate для синхронной сборки multipart-тела без tokio
    (правило «предпочитать crates»). Если подходящего нет, написать маленький кодировщик в
    `engine/openai.rs`: boundary, поля `model`, `response_format`, `language`,
    `timestamp_granularities[]` и один файл `audio/wav`, с тестом формы на `tiny_http`.
- TLS: после замены проверить `cargo tree -i aws-lc-sys`. Если его больше никто не тянет, из
  [windows-build.md](windows-build.md) и `env.ps1` можно убрать обход с `TEMP` в `target\tmp`,
  нужный только `aws-lc-sys`/`lzma-sys`. `lzma-sys` проверить отдельно (`cargo tree -i lzma-sys`).
  Обход убирать только если чистая сборка `cargo build -p mediaa` проходит без него.

Новый трейт `Engine` (единственная реализация `Connections` + `FakeEngine` в тестах; трейт
оправдан подменой внешнего эффекта):

```rust
pub trait Engine: Send + Sync + 'static {
    fn chat(&self, request: &ChatRequest, cancel: &AtomicBool,
            on_text: &mut dyn FnMut(&str) -> bool) -> Result<ChatAnswer, EngineError>;
    fn embeddings(&self, request: &EmbeddingsRequest, cancel: &AtomicBool)
        -> Result<Vec<Vec<f32>>, EngineError>;
    fn rerank(&self, request: &RerankRequest, cancel: &AtomicBool)
        -> Result<Vec<RerankResult>, EngineError>;
    fn transcribe(&self, request: &TranscribeRequest, cancel: &AtomicBool)
        -> Result<Vec<Phrase>, EngineError>;
    fn diarize(&self, request: &DiarizeRequest, cancel: &AtomicBool)
        -> Result<Vec<Turn>, EngineError>;
    fn voice_embeddings(&self, request: &VoiceRequest, cancel: &AtomicBool)
        -> Result<Vec<Vec<f32>>, EngineError>;
    fn models(&self, connection: &str) -> Result<Vec<ModelInfo>, EngineError>;
    fn check(&self, source: &ModelSource, role: ModelRole) -> Result<ModelCheck, EngineError>;
}
```

- `ChatRequest` получает `source: ModelSource` вместо `model: String` и `hf_token`, а также
  `thinking: Option<Thinking>`. `chat_thinking` сливается с `chat`. `device`, `n_ctx`,
  `priority`, `no_repeats` удаляются; `json_schema` передаётся как `response_format`
  (`json_schema`) — проверить, что сервер это принимает, при отказе возвращать ошибку, а не
  молча игнорировать.
- `Connections::new(connections: Vec<Connection>)` и `set_connections(...)`: подключения с
  ключами (ключи из keychain подставляет `agent/secrets` при загрузке, как сейчас для
  `Provider`).
- `health`, `status`, `set_budget`, `set_wanted`, `forget_loaded`, `vad_open`, `recognize`,
  `inspect`, `pull`, `delete_model`, `check_model` удаляются. `pull` для Ollama — отдельная
  функция `ollama::pull(connection, model, on_progress, cancel)` в `engine`, вызывается из UI
  моделей через `jobs::Job`.

### Где теперь живёт чат-цикл

Сейчас два мозга: `ProviderBrain` (HTTP) и `EngineBrain` (локальный). Оставить один —
`ProviderBrain`, переименовать в `ChatBrain`. HTTP-вызов он делает через `Engine::chat`, а не
напрямую через `providers::chat_completion_stream`, иначе `FakeEngine` не покрывает агента.
Для этого `ChatRequest` несёт `tools: Vec<ToolSpec>`, `tool_choice`, а `ChatAnswer` — разобранные
`tool_calls` и `reasoning`. Логику `Collected` (сборку дельт SSE) перенести в `engine/openai.rs`.
`EngineBrain`, `wait_reason`, `waiter` с ветвлением по `ModelSource`, `hf_token`,
`models::local_spec` удалить; `provider_wait` остаётся единственным ожиданием (слот
`ProviderCalls`). `scan::ProviderExtract` и `scan`-реализация для движка сливаются в одну.

### Расшифровка

`TranscribeRequest { source, path: PathBuf, from: f64, to: f64, language: Option<String> }`.
Результат — `Vec<Phrase>` со словами (`mediaa_core::transcript::{Phrase, Word}`) в абсолютном
времени записи.

- HTTP (`OpenAi`, `OpenAiCompatible`): `agent/transcribe.rs` режет запись на окна (например,
  600 с, перекрытие 2 с), каждое декодирует через `mediaa_media::preview::decode_mono` в 16 кГц
  mono, кодирует в WAV в памяти (`hound` уже в `core`) и шлёт `multipart` на
  `/audio/transcriptions` с `response_format=verbose_json`,
  `timestamp_granularities[]=word`/`segment`. Ограничение OpenAI — 25 МБ на файл: 600 с WAV
  16 кГц mono 16 бит ≈ 19 МБ. Слова из перекрытия дедуплицировать по времени. Курсор `t`
  (возобновление после остановки) сохраняется по границе окна, как сейчас по фразам.
- Command: шаблон, например
  `whisper-cli -m {model} -f {input} -l {language} -ojf -of {output_stem}`. Плейсхолдеры:
  `{input}` (временный WAV окна или исходный файл — флаг `whole_file` в подключении),
  `{output}`, `{output_stem}`, `{model}`, `{language}`, `{from}`, `{to}`. Формат вывода
  подключения: `WhisperJson` (whisper.cpp `-ojf`), `OpenAiJson` (verbose_json), `Srt`, `Vtt`.
  Читать файл `{output}` или stdout, если `{output}` не используется.
- VAD и `VAD_MODEL` удаляются: сегментацию делает сервер или CLI.

### Диаризация: куски, перекрытие и внешний отпечаток голоса

Обработка кусками и качество склейки сохраняются. Наружу выносятся только две операции,
которые сейчас делает sherpa-onnx: «найти реплики спикеров в куске» и «посчитать вектор голоса
по образцу». Всё остальное — чистый Rust и остаётся в приложении.

Как устроено сейчас (`agent/src/diarize.rs` + `engine/src/speech.rs`), сохранить без изменений:

- `plan(total, CHUNK_SECONDS = 480, OVERLAP_SECONDS = 15)` режет запись на куски;
  курсор задачи `Cursor { chunks: Vec<Chunk> }` хранит готовые куски, поэтому остановка и
  перезапуск продолжают со следующего куска.
- В каждом куске спикеры пронумерованы заново. `link()` склеивает их:
  - `overlap_links` — пары спикеров соседних кусков, которые одновременно говорят в зоне
    перекрытия не меньше `OVERLAP_LINK_SECONDS`, считаются одним человеком безусловно;
  - `merge_voices_linked(turns, voices, links, LINK_THRESHOLD = 0.75, LINK_MIN_SECONDS = 3.0)` —
    агломеративная склейка по косинусу векторов `Voice.embedding`, короткие спикеры уходят к
    ближайшему голосу;
  - `merge_voices_to(..., speakers)` — если пользователь задал число спикеров.
- `speech.rs::voices()` собирает для каждого спикера куска образец голоса — до
  `VOICE_SECONDS = 30` с, не больше `VOICE_TURN_SECONDS = 10` с из одной реплики — и считает
  по нему вектор.

Что меняется:

1. **Перенос чистой логики из `engine` в `agent`.** Новый модуль `agent/src/voices.rs`:
   `Voice`, `Cluster`, `merge`, `merge_voices`, `merge_voices_linked`, `merge_voices_to`,
   `unit`, `cosine`, `compact`, сбор образцов (бывшая `voices()` без sherpa) и все их тесты из
   `speech.rs` (`voices_merge_by_cosine_and_short_speakers_go_to_the_nearest`,
   `merging_renumbers_…`, `the_gaps_of_the_clustering_numbering_…`,
   `merging_does_not_depend_on_…`, `linked_voices_merge_whatever_their_cosine_says`,
   `merging_to_a_known_count_…`, `linked`). Код переносится как есть. `Diarization { turns,
   voices }` тоже переезжает в агент. `Engine` отдаёт только `turns`.
2. **Кусок → реплики.** `DiarizeRequest { source, path: PathBuf }`, где `path` — WAV куска
   16 кГц mono (декодирование как сейчас в `decode_range`, запись через `hound`) во временном
   каталоге `.ai/tmp/<task id>/`. Команда с форматом `Rttm` возвращает реплики во времени куска.
   `RTTM`-строки `SPEAKER <file> 1 <start> <duration> <NA> <NA> <label> <NA> <NA>` разбираются в
   `Turn`, метки спикеров нумеруются в порядке первого появления. Число спикеров в кусок не
   передаётся, как и сейчас (`speakers: None`): оно применяется в `link` ко всей записи.
   Примеры для подсказки в UI: `sherpa-onnx-offline-speaker-diarization` с
   segmentation-моделью pyannote 3.0, скрипт `pyannote.audio`, NeMo.
3. **Образцы → векторы.** Роль `VoiceEmbedding`: команда с форматом `Vectors`.
   `VoiceRequest { source, clips: Vec<PathBuf> }` — по одному WAV-образцу на спикера куска.
   Один запуск на кусок: плейсхолдер `{inputs}` разворачивается в отдельные аргументы по одному
   на файл. Вывод в stdout или `{output}` — JSON-массив векторов в том же порядке:
   `[[0.1, …], [0.3, …]]`. Число векторов не совпадает с числом файлов → ошибка. Векторы
   нормализуются `unit()` на нашей стороне. Примеры: `sherpa-onnx` speaker embedding
   extractor (3D-Speaker CAM++, та же модель, что сейчас по умолчанию), `pyannote/embedding`,
   SpeechBrain ECAPA. Обёртка для sherpa-onnx — `scripts/agent/voice_embeddings.py` (шаг 9a).
4. **Без роли `VoiceEmbedding`** образцы не собираются, `Voice.embedding` пустой, и `merge`
   склеивает только по `overlap_links` (это поведение уже есть: кластер без вектора не
   сравнивается по косинусу). Задача пишет в результат предупреждение `"warning": "speakers
   are linked only by the overlaps"`, UI диаризации показывает подсказку настроить отпечаток
   голоса.
5. **Без роли `Diarization`** — как сейчас: реплики без спикеров, предупреждение.
6. `Chunk`/`ChunkVoice` и курсор не меняются: вектор считается сразу после куска и сохраняется
   в курсоре, поэтому после перезапуска ничего не пересчитывается.
7. Временные WAV удаляются после каждого куска. Каталог `.ai/tmp/<task id>/` удаляется по
   завершении или отмене задачи.
8. `speakers::label`, `names.rs`, `transcript::replies` не меняются.
9. `diarize_quality.rs` и eval-гриды диаризации остаются. Это живые `#[ignore]`-замеры, теперь
   через подключения из env (шаг 7). Гриды по моделям sherpa превращаются в гриды по командам
   пользователя.

### Эмбеддинги и реранк

- `EmbeddingsRequest { source, input, kind }`. `kind` (`Query`/`Document`) превращается в
  префикс текста по данным пресета модели (сейчас его, скорее всего, делает `embed.rs` для
  embeddinggemma — перенести эту логику, найти её `grep -n "EmbedKind" crates/engine/src/embed.rs`).
- OpenAI-совместимые: `POST /embeddings`. Ollama: `POST /api/embed` (`{"model","input":[...]}`).
- Реранк: `POST {base}/rerank` с `{"model","query","documents","top_n"}`, ответ
  `results[].{index, relevance_score}`. Без реранкера поиск идёт как сейчас (`search.rs`
  уже это умеет).
- Смена модели эмбеддингов меняет размерность. Индекс должен хранить
  `connection/model` вместе с векторами и переиндексировать при несовпадении. Проверить, как
  это делается сейчас в `index.rs`/`core/retrieval.rs`, и сохранить поведение для новой строки
  источника (`ModelSource` → `"{connection}/{model}"`).

### Бюджет и параллелизм

- `Budget` в `core`: удалить `gpu_memory_gb`, `ram_memory_gb`, `keep_alive_secs`, `gpu_bytes`,
  `ram_bytes`, `gb`. Лимиты по `WorkKind` остаются — это `Slots`, они ограничивают число
  одновременных запросов к подключениям и процессов CLI.
- `Device`, `ModelDevices`, `AgentSettings.devices` удалить.
- `mediaa_engine::Budget`, `engine_budget` в `AgentController`, `set_budget` удалить.

### Настройки (`core/src/agent.rs`)

```rust
pub enum ConnectionKind { OpenRouter, OpenAi, DeepSeek, HuggingFace, Ollama, OpenAiCompatible, Command }

pub struct Connection {
    pub id: String,
    pub name: String,
    pub kind: ConnectionKind,
    #[serde(default)] pub base_url: String,
    #[serde(skip)] pub api_key: String,
    #[serde(default)] pub proxy: String,
    #[serde(default = "default_context")] pub context: u32,
    #[serde(default)] pub command: Option<CommandSpec>,
}

pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub output: OutputFormat,
    #[serde(default)] pub whole_file: bool,
}

pub enum OutputFormat { WhisperJson, OpenAiJson, Srt, Vtt, Rttm, Vectors }

pub struct ModelSource { pub connection: String, pub model: String }
```

- `Connection::api_url()` берёт URL из пресета для фиксированных видов и `base_url` для
  `Ollama`/`OpenAiCompatible` (`Ollama` → `{base_url}/v1` для chat и `{base_url}` для `/api/*`).
- `Connection::capabilities()` — таблица выше.
- `AgentSettings.providers` → `connections`; `settings.provider(id)` → `connection(id)`.
- `thinking: Vec<ModelSource>` и `thinking_on` сохраняются.
- `paths::models_dir` удалить.
- Старые сохранённые `[agent]` с `source = "hugging_face"` перестанут разбираться. По правилам
  миграции нет. `Settings::load_from` при ошибке сбрасывает все настройки к умолчанию — см.
  «Риски».

### UI (`crates/app/src/pages/agent`)

- `providers.rs` → страница «Подключения»: список, добавление по виду (пресеты: OpenRouter,
  OpenAI, DeepSeek, Hugging Face, Ollama, OpenAI-совместимый, Команда), поля по виду, ключ
  (keychain), прокси, кнопка «Проверить». Для Ollama проверка показывает: найден ли `ollama`,
  отвечает ли демон, кнопка «Запустить ollama serve», если не отвечает. Для Команды: программа,
  аргументы (по одному в строке), формат вывода, «целый файл», подсказка с плейсхолдерами и
  примерами whisper.cpp / sherpa-onnx / pyannote.
- `models.rs`: для каждой роли — выбор подключения (только с нужной возможностью) и модели.
  Роли «Диаризация» и «Отпечаток голоса» стоят рядом. Если выбрана диаризация без отпечатка,
  показывается подсказка про склейку только по перекрытию.
  Модель выбирается из списка `Engine::models(connection)` (`GET /models`, Ollama `/api/tags`) с
  поиском или вводится вручную. Для Ollama модель, которой нет в `/api/tags`, получает кнопку
  «Скачать (ollama pull)» с прогрессом через `jobs::Job`. Удалить inspect/pull/delete HF,
  «папку моделей», `Download`, `Fetch`/`fetch_next`/`fetch_line`, `repository_rows`, `fits`,
  `format_name`, `is_downloaded`, `repo_id`, `models_folder`.
- `budget.rs`: оставить только лимиты параллелизма.
- `engine_state.rs`: удалить (нет загруженных моделей и GPU). Его места в `mod.rs`/`studio.rs`/
  `queue.rs` убрать. Ожидание показывается только как «ждёт слот подключения».
- `chat.rs:1493`, `dialog.rs`, `task_details.rs`, `queue.rs` — заменить `Resource::Provider`/
  `Resource::Local` на одно значение. Удалить ветки, показывающие загрузку модели.
- `features/agent.rs`: `AgentController::new(engine, ...)` без бюджета памяти; при изменении
  `connections` вызывать `Connections::set_connections`. Опрос `status` удалить.
- `app.rs:170`: вместо `LocalEngine::new(paths::models_dir(), …)` —
  `Connections::new(settings.agent.connections.clone())`.
- Locale: удалить ключи `budget.gpu_memory*`, `budget.ram_memory*`, `budget.keep_alive*`,
  `agent.inspect*`, ключи загрузки/папки моделей и состояния движка. Добавить ключи подключений
  и команд. Тест на неизвестные/лишние ключи (`i18n.rs`) должен проходить.

### Сборка и документация

- `.cargo/config.toml`: удалить `LLAMA_STATIC_CRT`, `CMAKE_GENERATOR`, `CMAKE_TOOLCHAIN_FILE`.
  Оставить `FFMPEG_DIR` и `crt-static`.
- Удалить `cmake/static-crt.cmake`, `Directory.Build.targets`.
- `scripts/windows/env.ps1`: удалить проверки CMake и Vulkan SDK. Оставить vcpkg, libclang и
  TEMP в `target\tmp` — он нужен `aws-lc-sys`/`lzma-sys`, не llama. Проверить, нужен ли ещё
  libclang: `cargo tree -i bindgen`; если без llama/sherpa его никто не тянет, кроме ffmpeg-sys, —
  оставить.
- `Cargo.toml`: вернуть профилю `dev` debug info (убрать `[profile.dev] debug = 0`), если
  `cargo build -p mediaa` проходит без D8050. Причина D8050 — сборка llama.cpp.
- README: убрать CMake/Vulkan из требований, добавить раздел «Модели агента»: что поставить
  пользователю (Ollama, ключ OpenRouter/OpenAI/DeepSeek, whisper.cpp или OpenAI для расшифровки,
  pyannote/sherpa-onnx CLI для диаризации, обёртка `scripts/agent/voice_embeddings.py` для
  отпечатка голоса) и примеры команд.
- `specs/architecture.md` §8 и §7.2.5–7.2.6, `specs/constraints.md` (удалить Vulkan/CMake и
  D8050, добавить: «инференс только внешний; процессы без оболочки; OpenAI audio ≤ 25 МБ»),
  [windows-build.md](windows-build.md), [agent-port.md](agent-port.md) — синхронизировать.

## Пошаговый план

Каждый шаг оставляет workspace компилируемым: `cargo check --workspace --all-targets --profile
dev-check`. Тесты удалённого поведения удаляются в том же шаге, что и поведение.

1. **Настройки.** В `core/src/agent.rs` ввести `Connection`, `ConnectionKind`, `CommandSpec`,
   `OutputFormat`, `Capability`, `capabilities()`, новый `ModelSource`. Удалить `Provider`,
   `ProviderKind`, `Device`, `ModelDevices`, поля памяти `Budget`, HF-дефолты, `ModelRole::Speech`
   (после проверки). Убрать `paths::models_dir`. Обновить тесты `core`
   (`thinking_on`, сериализация настроек). Потребителей пока не трогать: зафиксировать список
   ошибок компиляции через `cargo check`.
2. **Подключения в engine.** Создать `openai.rs` на `ureq` (перенести `agent/src/providers.rs`
   и сборку SSE из `assistant/provider.rs::Collected`; один `ureq::Agent` на подключение), `ollama.rs`, `command.rs`, `presets.rs`, новый
   `Engine` и `Connections`, новые типы. До удаления `speech.rs` перенести его чистую логику
   склейки голосов с тестами в `agent/src/voices.rs` (см. «Диаризация», п. 1). Удалить
   локальный runtime и его зависимости из `crates/engine/Cargo.toml`. Переписать `FakeEngine` под новый трейт (записывает chat с tools,
   embeddings, rerank, transcribe, diarize; отдаёт заданные ответы). Тесты:
   - `openai.rs`: SSE-сервер на `tiny_http` (как в `assistant/provider.rs` тестах) —
     стрим текста, tool_calls по индексам, reasoning (`reasoning_content` и `reasoning`), usage,
     401/429, отмена посреди стрима; embeddings; rerank; multipart transcriptions
     (проверить поля формы и разбор verbose_json со словами);
   - `ollama.rs`: `/api/tags`, `/api/embed` на фейковом сервере; поиск `ollama` в PATH;
   - `command.rs`: фейковая программа — маленький тестовый бинарь в `tests/` или
     `std::env::current_exe` с env-флагом, печатающий заданный вывод; разбор каждого
     `OutputFormat`; подстановка плейсхолдеров; ненулевой код возврата → ошибка с stderr;
     отмена убивает процесс.
2a. **Веб-поиск без reqwest.** `agent/src/web.rs` перевести на `ureq`, удалить `reqwest` из
   `agent/Cargo.toml`. Тесты `web.rs` на `tiny_http` должны проходить без изменений ожиданий.
   После шага: `cargo tree -i tokio --workspace` и `cargo tree -i reqwest --workspace` пусты.
3. **Агент: один мозг.** `ProviderBrain` → `ChatBrain` поверх `Engine::chat`. Удалить
   `EngineBrain`, `with_brain`-ветвление, `waiter`/`wait_reason`, `hf_token`, `models.rs`,
   `providers.rs`. Перевести `outline.rs`, `names.rs`, `scan.rs`, `title.rs`,
   `step_title.rs` на `ChatRequest { source, ... }`. `check.rs` → `Engine::check`. Перенести
   тесты `assistant/provider.rs` на `FakeEngine` (поведение цикла) и на `openai.rs` (формат HTTP).
4. **Агент: эмбеддинги/реранк/индекс.** `search.rs`, `index.rs` на новые запросы; ключ
   источника индекса `"{connection}/{model}"`; тест: смена модели эмбеддингов вызывает
   переиндексацию.
5. **Агент: расшифровка.** Новый `transcribe.rs` на окнах + `Engine::transcribe`;
   дедупликация слов в перекрытии; курсор по окнам. Тесты на `FakeEngine`: окна покрывают всю
   запись, перекрытие не дублирует слова, остановка и продолжение с того же окна, ошибка окна
   проваливает задачу с понятным текстом.
6. **Агент: диаризация кусками.** `voices.rs` уже перенесён на шаге 2. `diarize.rs`: кусок → WAV → `Engine::diarize` → `Turn`; если задана роль `VoiceEmbedding` —
   образцы → `Engine::voice_embeddings` → `ChunkVoice.embedding`; `link` как сейчас. Тесты на
   `FakeEngine`:
   - сохранить существующие тесты склейки (`the_same_voice_in_two_chunks_is_one_speaker`,
     `a_short_voice_of_one_chunk_goes_to_its_nearest_voice`,
     `talking_at_once_in_the_overlap_links_the_chunks_whatever_the_voices_say`,
     `chunks_overlap_and_cover_the_whole_recording`,
     `a_stopped_diarization_goes_on_from_the_next_chunk_with_the_same_replies`,
     `diarize_a_long_video_in_chunks_across_a_restart`);
   - новые: без `VoiceEmbedding` спикер, говорящий в перекрытии, склеивается, а результат
     несёт предупреждение; в `voice_embeddings` уходит по одному образцу на спикера куска,
     каждый не длиннее 30 с; несовпадение числа векторов → ошибка куска; после перезапуска
     векторы готовых кусков не запрашиваются повторно; временные WAV удалены;
   - `command.rs`: разбор RTTM (метки в порядке появления, `duration` → `end`), разбор
     `Vectors`, разворачивание `{inputs}`.
7. **Eval.** `crates/eval`: `LocalEngine` → `Connections` из настроек eval-кейса. DER в
   `score.rs` и `diarize_quality.rs` остаются, считаются по результату `link`. `live.rs` в agent
   и живые замеры диаризации (`#[ignore]`) берут подключения из env (`MEDIAA_LIVE_CONNECTIONS`
   — путь к toml с `connections` и `models`), без `models_dir`. В отчёте шага записать DER
   на `evals/data/librivox-mix-1` через sherpa-onnx CLI + обёртку CAM++ и сравнить с прежним
   встроенным вариантом. Если DER заметно хуже, искать расхождение в настройках
   segmentation/кластеризации CLI, а не менять склейку.
8. **UI.** Страницы «Подключения», «Модели», «Бюджет» по разделу «UI»; удалить
   `engine_state.rs`; `features/agent.rs` и `app.rs`; `app/test_support.rs` — `FakeEngine` без
   бюджета. Locale en/ru. kittest: добавить подключение каждого вида, выбор модели роли из
   списка `FakeEngine::models`, кнопка `ollama pull` запускает job, роль не предлагает
   подключение без нужной возможности. Snapshot `agent-catalogs-wide` и затронутые обновить
   только после просмотра diff (`UPDATE_SNAPSHOTS=true cargo test -p mediaa --lib
   ui_regression_snapshot`).
9. **Сборка.** Пункт «Сборка и документация»: config, cmake, targets, env.ps1, профиль dev.
   Проверить `cargo tree -i llama-cpp-sys-2` и `cargo tree -i sherpa-onnx-sys` — пусто.
   Чистая сборка `cargo build -p mediaa` без `VULKAN_SDK` в окружении.
9a. **Скрипты-обёртки sherpa-onnx** в `scripts/agent/` (Python 3.10+, зависимости
    `sherpa-onnx`, `soundfile`, `numpy`; приложение их не запускает само, пользователь
    указывает их в подключении «Команда», см. [docs/agent-models.md](../../docs/agent-models.md)):
    - `sherpa_transcribe.py --model M --tokens T [--language L] INPUT` — NeMo CTC
      (`OfflineRecognizer.from_nemo_ctc`; флаг `--kind` для transducer/whisper-моделей
      sherpa-onnx), печатает OpenAI `verbose_json`: `text`, `segments[]`, `words[]` с
      `start`/`end` в секундах. Слова собираются из токенов и `timestamps` так же, как сейчас
      `engine/src/speech.rs::words` — перенести эту функцию в скрипт.
    - `sherpa_diarize.py --segmentation S --embedding E [--threshold 0.5] INPUT` —
      `OfflineSpeakerDiarization`, печатает RTTM.
    - `voice_embeddings.py --model E INPUT...` — `SpeakerEmbeddingExtractor`, печатает
      JSON-массив векторов в порядке входных файлов.
    Каждый скрипт пишет ошибки в stderr и возвращает ненулевой код. Тест: живой `#[ignore]`
    в `command.rs` или `live.rs` прогоняет каждый скрипт на `evals/data` и проверяет разбор
    вывода; в CI не входит.
10. **Документация.** README (ссылка на `docs/agent-models.md` в разделе Features/Agent),
    сверка `docs/agent-models.md` с итоговым UI (названия, аргументы, форматы), удаление из
    него вводного предупреждения о том, что задача ещё не выполнена, `architecture.md`, `constraints.md`, `windows-build.md`,
    `agent-port.md`, этот файл (раздел «Выполнено»/«Проверка»), статус в `progress.md`.

## Проверка

- `cargo fmt --all -- --check`.
- `cargo check --workspace --lib --bins --profile dev-check`, затем
  `cargo clippy --workspace --all-targets -- -D warnings`.
- `cargo test -p mediaa-core -p mediaa-engine -p mediaa-agent -p mediaa-queue -p mediaa-eval`.
- `cargo test -p mediaa --lib -- --test-threads=1` (UI; GPU-нестабильность — см.
  [agent-port.md](agent-port.md)).
- `cargo tree -p mediaa | findstr /i "llama sherpa hf-hub safetensors tokio reqwest"` — пусто.
- Сборка `mediaa` в окружении без `VULKAN_SDK` и без CMake в PATH.
- Ручная проверка в отдельном экземпляре (`MEDIAA_DEV_LIBRARY`, `MEDIAA_INSTANCE_ID`,
  `--page=`): подключение Ollama с установленным демоном — список моделей, чат агента; OpenRouter
  с ключом пользователя — чат; расшифровка через whisper.cpp CLI или OpenAI на короткой записи.
  Ключи вводит пользователь сам.

## Материальные риски

- **Сброс настроек.** `Settings::load_from` при любой ошибке разбора возвращает
  `Settings::default()` целиком. Старый `[agent]` с удалёнными вариантами `ModelSource` сбросит и
  неагентские настройки. Ветка ещё не выпущена, поэтому миграцию по правилам не пишем, но
  разработчик должен перед запуском удалить секцию `[agent]` из файла настроек. Указать это в
  отчёте.
- **Качество диаризации** внутри куска зависит от внешнего инструмента; склейка между
  кусками остаётся нашей. Без роли `VoiceEmbedding` спикер, молчавший в перекрытии, может
  получить новый номер в следующем куске. Это видно по предупреждению задачи.
- **Векторы разных моделей несовместимы.** Смена команды отпечатка голоса посреди задачи
  смешала бы векторы. Хранить в курсоре `Chunk` строку источника `VoiceEmbedding`; если она не
  совпадает с текущей, пересчитать векторы готовых кусков (WAV куска декодируется заново).
- **Запуск процессов на каждый кусок** (2 процесса на 8 минут записи) добавляет время загрузки
  модели CLI. Для часа записи это ~16 запусков — приемлемо. Отдельный долгоживущий сервер
  отпечатков не делаем без замера.
- **Совместимость серверов.** `response_format: json_schema`, `tools`, `/rerank` и
  `timestamp_granularities` поддерживаются не везде. Проверка подключения должна делать
  минимальный реальный вызов каждой нужной возможности и показывать ошибку сервера как есть.
- **Ollama `pull`** может идти долго. Нужны прогресс и отмена: при CLI — парсинг вывода
  `ollama pull`, при HTTP — стрим `POST /api/pull`. Предпочтительно HTTP `/api/pull`: он даёт
  JSON-прогресс без разбора терминального вывода. CLI оставить для `--version` и `serve`.
- **Окна расшифровки по HTTP**: границы окна могут резать слово. Перекрытие и дедупликация
  по времени это компенсируют; тест на это обязателен.
- **`ureq` против `reqwest`**: другое поведение ошибок соединения и таймаутов. Тексты ошибок
  для пользователя (ключ отклонён, 429, недоступен сервер, прокси) закрепить тестами до замены и
  проверить после неё.
- **Linux/macOS** не проверялись и раньше. После удаления нативного инференса сборка на них
  должна упроститься, но проверить её здесь нечем.
