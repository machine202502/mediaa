# Модели для агента MediaA: что установить на свой компьютер

> Приложение не содержит собственных моделей и движков. Подключение описывается в Agent →
> Подключения, роль — в Agent → Модели. План задачи:
> [agent-external-models.md](../specs/tasks/agent-external-models.md).

В путях ниже `~` означает вашу домашнюю папку (`C:\Users\<вы>` в Windows, `/home/<вы>` в
Linux). MediaA понимает `~` и в полях подключения «Команда».

MediaA не хранит своих моделей и ничего не запускает в фоне само. Единственное исключение —
кнопка «Скачать» для Ollama, которая просит вашу Ollama скачать модель. Модели ставите вы — так, как удобно:
облачный сервис по ключу, Ollama, свой сервер llama.cpp, программы на ONNX. В MediaA вы только
описываете **подключение** (Agent → Подключения) и выбираете модель для каждой **роли**
(Agent → Модели).

## Какие роли бывают и чем их закрыть

| Роль | Зачем | Чем закрыть |
|---|---|---|
| Агент | отвечает в чате, пользуется инструментами | OpenRouter, OpenAI, DeepSeek, Ollama, llama.cpp-сервер |
| Заголовки | короткие названия чатов и шагов | любая маленькая чат-модель (Ollama, облако) |
| Эмбеддинги | поиск по записям | Ollama, OpenAI, OpenRouter, llama.cpp-сервер |
| Реранкер (необязательно) | точнее сортирует найденное | llama.cpp-сервер, vLLM, LocalAI |
| Расшифровка | речь → текст со временем слов | GigaAM (sherpa-onnx), whisper.cpp, OpenAI |
| Диаризация (необязательно) | кто когда говорил | sherpa-onnx + pyannote segmentation |
| Отпечаток голоса (необязательно, но нужен для хорошей диаризации) | узнавать того же спикера в разных кусках записи | sherpa-onnx + CAM++ |

Самый короткий путь для старта:
1. **Чат и эмбеддинги** — Ollama (раздел 2) или ключ OpenRouter (раздел 1).
2. **Русская расшифровка** — GigaAM v3 от Сбера через sherpa-onnx (раздел 4).
3. **Спикеры** — диаризация и отпечаток голоса через sherpa-onnx (раздел 5).

---

## 0. Общая подготовка

### Python (нужен для ONNX-моделей из разделов 4–5)

- **Windows:** установите Python 3.10 или новее с [python.org](https://www.python.org/downloads/)
  и при установке отметьте «Add python.exe to PATH». Проверка: `python --version`.
- **Linux (Debian/Ubuntu):** `sudo apt install python3 python3-venv python3-pip`.
  Fedora: `sudo dnf install python3 python3-pip`.
- **macOS:** `brew install python`.

Держите модели и программы для MediaA в отдельном виртуальном окружении, чтобы они не
конфликтовали с другими программами:

```bash
python -m venv ~/mediaa-models/venv
```

Активация окружения: в Windows PowerShell —
`~/mediaa-models/venv/Scripts/Activate.ps1`, в Linux/macOS —
`source ~/mediaa-models/venv/bin/activate`.

В MediaA потом указывается полный путь к Python из этого окружения:
`C:\Users\<вы>\mediaa-models\venv\Scripts\python.exe` или
`/home/<вы>/mediaa-models/venv/bin/python`.

### Утилита для скачивания моделей с Hugging Face

```bash
pip install -U "huggingface_hub[cli]"
```

Команда `hf download <репозиторий> --local-dir <папка>` скачивает модель в папку. Для закрытых
моделей сначала выполните `hf auth login`.

### Видеокарта

- **Ollama** сама находит NVIDIA (CUDA), AMD (ROCm) и Apple Silicon (Metal). Достаточно
  свежего драйвера.
- **llama.cpp** на Windows/Linux с любой видеокартой работает через Vulkan. Нужен только
  драйвер с поддержкой Vulkan. Vulkan SDK нужен, только если вы собираете llama.cpp сами.
  - Linux: `sudo apt install mesa-vulkan-drivers vulkan-tools` (AMD/Intel) или проприетарный
    драйвер NVIDIA. Проверка: `vulkaninfo --summary`.
- **sherpa-onnx** из разделов 4–5 по умолчанию работает на процессоре. Для GigaAM и
  диаризации этого хватает: час записи обрабатывается за минуты.

---

## 1. Облачные модели: OpenRouter, OpenAI, DeepSeek

Устанавливать ничего не нужно, нужен только ключ.

| Сервис | Где взять ключ | Что даёт |
|---|---|---|
| OpenRouter | [openrouter.ai/keys](https://openrouter.ai/keys) | сотни моделей одним ключом (Claude, GPT, Gemini, Qwen, DeepSeek…), эмбеддинги |
| OpenAI | [platform.openai.com/api-keys](https://platform.openai.com/api-keys) | чат, эмбеддинги, расшифровка (`gpt-4o-transcribe`, `whisper-1`) |
| DeepSeek | [platform.deepseek.com](https://platform.deepseek.com/) | `deepseek-chat`, `deepseek-reasoner` |

В MediaA: Agent → Подключения → «Добавить» → вид сервиса → вставить ключ → «Проверить». Ключ
хранится в системном хранилище паролей (Windows Credential Manager, macOS Keychain, Secret
Service в Linux), а не в файле настроек.

В Linux для хранения ключа нужен работающий Secret Service: GNOME Keyring или KWallet. В
большинстве рабочих окружений он уже есть. На «голом» оконном менеджере поставьте
`gnome-keyring`.

Если сервис доступен только через прокси, укажите его в поле «Прокси» подключения
(`http://…`, `socks5://…`).

---

## 2. Ollama — локальные модели одной командой

### Установка

- **Windows:** установщик с [ollama.com/download](https://ollama.com/download). После установки
  Ollama работает в трее.
- **Linux:**
  ```bash
  curl -fsSL https://ollama.com/install.sh | sh
  ```
  Установщик ставит службу `ollama`. Проверка: `systemctl status ollama`.
- **macOS:** приложение с [ollama.com/download](https://ollama.com/download) или
  `brew install ollama`.

Проверка: `ollama --version` и `ollama list`.

### Модели

```bash
ollama pull qwen3:8b            # агент, хорошо говорит по-русски
ollama pull qwen3:1.7b          # заголовки — маленькая и быстрая
ollama pull embeddinggemma      # эмбеддинги для поиска
ollama pull bge-m3              # эмбеддинги, сильная многоязычная альтернатива
```

Размер модели подбирайте под память: 4B-модели нужно около 3–4 ГБ, 8B — около 6 ГБ, 14B —
около 10 ГБ видеопамяти (или обычной памяти, если видеокарты нет). Каталог моделей —
[ollama.com/library](https://ollama.com/library).

### В MediaA

Подключения → «Ollama». Адрес по умолчанию — `http://localhost:11434`. Модели Ollama
появятся в списке ролей. Модель, которой ещё нет, можно скачать кнопкой «Скачать» прямо в
MediaA.

Если Ollama стоит на другом компьютере в сети, запустите её там с
`OLLAMA_HOST=0.0.0.0` и укажите в MediaA адрес `http://<ip>:11434`.

---

## 3. llama.cpp-сервер — свои GGUF-модели и Vulkan

Это вариант, если вы хотите точно выбирать файл модели и квантизацию, нужен реранкер или
не подходит Ollama.

### Установка

- **Windows:** скачайте архив с [github.com/ggml-org/llama.cpp/releases](https://github.com/ggml-org/llama.cpp/releases):
  `…-bin-win-vulkan-x64.zip` для любой видеокарты или `…-bin-win-cuda-…` для NVIDIA. Распакуйте,
  например, в `C:\llama.cpp`.
- **Linux:** готовые сборки есть там же (`…-bin-ubuntu-vulkan-x64`). Или соберите сами:
  ```bash
  sudo apt install build-essential cmake git libvulkan-dev glslc
  git clone https://github.com/ggml-org/llama.cpp && cd llama.cpp
  cmake -B build -DGGML_VULKAN=ON
  cmake --build build --config Release -j
  ```
  Программы окажутся в `build/bin/`.
- **macOS:** `brew install llama.cpp` (работает через Metal).

### Запуск

Чат-модель (llama.cpp сам скачает GGUF с Hugging Face):

```bash
llama-server -hf unsloth/Qwen3-8B-GGUF:Q4_K_M --port 8080 --jinja -c 32768
```

Эмбеддинги и реранкер — отдельными процессами на других портах:

```bash
llama-server -hf ggml-org/embeddinggemma-300M-GGUF --port 8081 --embedding
llama-server -hf gpustack/bge-reranker-v2-m3-GGUF --port 8082 --reranking
```

`--jinja` нужен, чтобы агент мог вызывать инструменты. Флаг `-ngl 99` переносит все слои на
видеокарту, если llama.cpp не сделал этого сам.

### В MediaA

На каждый запущенный сервер — подключение «OpenAI-совместимый» с адресом
`http://localhost:8080/v1` (`8081`, `8082` для остальных). Ключ не нужен.

Так же подключаются **LM Studio** (`http://localhost:1234/v1`), **vLLM**, **LocalAI** и любые
другие серверы с OpenAI-совместимым API.

---

## 4. Расшифровка речи

### 4.1. GigaAM v3 от Сбера (русский язык, ONNX, sherpa-onnx) — рекомендуется

GigaAM — модель распознавания русской речи от SberDevices. Готовая ONNX-версия с пунктуацией
лежит на Hugging Face в формате [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx).

1. Установите sherpa-onnx в окружение из раздела 0:
   ```bash
   pip install sherpa-onnx soundfile numpy
   ```
2. Скачайте модель:
   ```bash
   hf download csukuangfj/sherpa-onnx-nemo-ctc-punct-giga-am-v3-russian-2025-12-16 --local-dir ~/mediaa-models/gigaam-v3
   ```
   В папке будут `model.onnx` (или `model.int8.onnx` — меньше и быстрее) и `tokens.txt`.
3. Свой сервер расшифровки должен принять файл и вернуть текст. GigaAM сама не ставит
   спикеров и не считает отпечаток. Для разработки в корне репозитория есть
   `docker compose up`: чат на порту 8080, эмбеддинги на 8081, расшифровка с диаризацией
   и отпечатком на 8293 (ключ `mediaa`). Пользователю этот контейнер поднимать не нужно.

Другие модели sherpa-onnx (английский, мультиязычные, Whisper в ONNX) подключаются так же.
Список — [k2-fsa.github.io/sherpa/onnx/pretrained_models](https://k2-fsa.github.io/sherpa/onnx/pretrained_models/index.html).

### 4.2. whisper.cpp (много языков)

- Windows: архив `whisper-bin-x64.zip` (или `-cublas-` для NVIDIA) с
  [github.com/ggml-org/whisper.cpp/releases](https://github.com/ggml-org/whisper.cpp/releases).
- Linux/macOS: `brew install whisper-cpp` или сборка
  `cmake -B build -DGGML_VULKAN=ON && cmake --build build -j`.
- Модель: `hf download ggerganov/whisper.cpp ggml-large-v3-turbo.bin --local-dir ~/mediaa-models/whisper`.

В MediaA — «Команда»: программа `whisper-cli`, аргументы
`-m ~/mediaa-models/whisper/ggml-large-v3-turbo.bin`, `-f`, `{input}`, `-l`, `{language}`,
`-ojf`, `-of`, `{output_stem}`; формат вывода — «whisper.cpp JSON».

### 4.3. OpenAI (облако)

Подключение OpenAI из раздела 1, модель `gpt-4o-transcribe` или `whisper-1`. MediaA сама режет
запись на куски, которые укладываются в лимит OpenAI в 25 МБ на файл.

---

## 5. Спикеры: диаризация и отпечаток голоса (sherpa-onnx)

MediaA обрабатывает запись кусками по 8 минут. Диаризация находит реплики внутри куска, а
отпечаток голоса позволяет понять, что «спикер 2» в одном куске и «спикер 1» в другом — один
человек. Без отпечатка MediaA склеивает спикеров только по перекрытию кусков, и это работает
хуже.

1. sherpa-onnx уже установлен в разделе 4.1. Если нет — `pip install sherpa-onnx soundfile numpy`.
2. Скачайте модели:
   ```bash
   hf download csukuangfj/sherpa-onnx-pyannote-segmentation-3-0 --local-dir ~/mediaa-models/segmentation
   hf download csukuangfj/speaker-embedding-models 3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx --local-dir ~/mediaa-models/voices
   ```
3. Обёртка над этими моделями должна отдавать реплики и, если умеет, вектор голоса.
   Готовые скрипты в репозиторий не входят. Тот же набор для разработки поднимает
   `docker compose up` из корня: порт 8293.

Альтернатива — [pyannote.audio](https://github.com/pyannote/pyannote-audio). Он точнее на
сложных записях, но требует PyTorch, токен Hugging Face и принятия лицензии модели. Подключается
своим скриптом, который печатает RTTM.

---

## 6. Проверка

В Agent → Подключения у каждого подключения есть кнопка «Проверить». Она делает настоящий
короткий вызов: для чата — одно сообщение, для эмбеддингов — один вектор, для команды — запуск
на секундном тестовом звуке. Ошибка показывается так, как её вернул сервер или программа.

Частые проблемы:

| Симптом | Причина и что делать |
|---|---|
| «ключ отклонён» | ключ неверный или закончились средства на счёте сервиса |
| Ollama «не отвечает» | служба не запущена: Windows — запустите Ollama из меню «Пуск»; Linux — `sudo systemctl start ollama` |
| llama-server: модель не помещается | возьмите квантизацию меньше (`Q4_K_M` → `Q3_K_M`) или уменьшите `-c` |
| команда: «не найдена программа» | укажите полный путь к `python`/`whisper-cli`, а не просто имя |
| команда: `ModuleNotFoundError: sherpa_onnx` | в «программе» указан не тот Python — нужен Python из окружения раздела 0 |
| спикеры путаются между кусками | не настроен «Отпечаток голоса» (раздел 5, шаг 4) |
| Linux: ключ не сохраняется | нет Secret Service — установите `gnome-keyring` |
