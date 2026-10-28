# Перенос фичи агента на актуальный main

## Цель

Перенести фичу агента MediaA из старой ветки `feature/agent` на актуальную архитектуру `main`
целиком, без частичного переноса, и привести ветку `feature/agent` к состоянию, пригодному для
мержа в `main`.

## Контекст

`feature/agent` была основана на старой монолитной архитектуре: весь `core` в одном крейте,
страницы `library`/`convert` в `app`, документация в `workflow/`. `main` к этому моменту уже
разбил ядро на `storage`, `render`, `webp-pipeline`, `media`, `platform`, перевёл `app` на owned
lifecycles (`features/*`, `app/state`, `app/command`) и заменил `workflow/` на `specs/`.

Исходная фича агента — это один сквош-коммит `19ddf88` поверх старой базы. Новые крейты
`agent`, `engine`, `queue`, `eval` и агент-модули `core` почти самодостаточны; адаптации
потребовал UI (`pages/agent`, `tab_strip`) и интеграция в новый `app`.

## Ограничения

- Не переносить старую архитектуру обратно: не возвращать `workflow/`, старые
  `pages/library.rs`/`pages/convert.rs`/`player.rs`, grok/subagent-скрипты и монолитные
  крейты.
- Фича агента должна попасть целиком: все крейты, core-модули, UI, локали, настройки.
- Ключи API не хранятся в файле настроек — только в OS keychain (`core/secrets`).

## Решение

1. Резервная копия старой ветки в `feature/agent-temp`, сброс `feature/agent` на `main`.
2. В `core` добавлены модули `agent`, `catalogs`, `chat`, `outline`, `pulse`, `retrieval`,
   `secrets`, `speakers`, `transcript`; в `Settings` добавлено поле `agent`, в `paths` —
   `models_dir`. Запись настроек (`SettingsWriter`) сохранена через `persist_with_secrets`.
3. Добавлены workspace-крейты `agent`, `engine`, `queue`, `eval` и их workspace-зависимости.
4. UI агента интегрирован в новый `app`: страницы `Catalogs`/`Providers`/`Models`/`Queue`/
   `Budget`, секция `Agent`, `tab_strip`, адаптация `Player` к
   `features::library::player::Player`, замены `library_dir` на `settings.media_dir`.
5. Добавлены locale-ключи агента в `en.toml`/`ru.toml` без удаления существующих ключей.
6. Нативные build-файлы (`Directory.Build.targets`, `cmake/static-crt.cmake`, `.cargo/config.toml`).

## Затронутые файлы

- `Cargo.toml`, `crates/core/Cargo.toml`, `crates/app/Cargo.toml`, `.cargo/config.toml`,
  `.gitattributes`, `Directory.Build.targets`, `cmake/static-crt.cmake`
- `crates/core/src/*` (новые агент-модули, `lib.rs`, `settings.rs`, `paths.rs`)
- `crates/{agent,engine,queue,eval}/**`, `evals/**`
- `crates/app/src/pages/agent/**`, `crates/app/src/tab_strip.rs`
- `crates/app/src/{app.rs,app/state.rs,pages/mod.rs,lib.rs,theme.rs,services/settings_writer.rs}`
- `crates/app/locales/{en,ru}.toml`, `specs/architecture.md`

## Состояние после аудита 27.10.26

HEAD `feature/agent` совпадает с `main`; перенос существует только как незакоммиченные изменения.
Фактическая проверка:

- `cargo check --workspace --lib --bins --profile dev-check` доходит до `mediaa` и падает на 6
  ошибках: приватный `features::library::player` в четырёх страницах агента, конфликт
  заимствований в `pages/agent/catalogs.rs`, перемещение в `FnMut` в `theme.rs`.
- Тестовая конфигурация не собирается: `App` получил поля `agent` и `last_agent_page`, а
  `app/test_support.rs` их не заполняет.
- После появления `Section::Agent` изменилась шапка, но 22 snapshot-baseline не обновлены;
  app-level UI-тестов и снимков агента нет.
- `cargo test -p mediaa-core -p mediaa-queue` проходит; тестовые таргеты `agent`, `engine`,
  `eval` компилируются.
- Архитектурные риски: `AgentUi` пересоздаёт scheduler при изменении бюджета, движок и очередь
  стартуют у всех пользователей, keychain вызывается из `Settings::load/persist`, язык агента
  берётся как `settings.language.unwrap_or("en")`, UI использует собственные потоки вместо
  `jobs`/`latest_worker`.
- Build-конфигурация содержит машинно-специфичные обходы: `CMAKE_GENERATOR` с VS 18 2026,
  глобальный `CMAKE_TOOLCHAIN_FILE`, `Directory.Build.targets`; Vulkan SDK/CMake не описаны в
  README и не проверяются `env.ps1`.

## План завершения

1. Собрать `mediaa`: видимость `player`, заимствование в `catalogs`, `theme.rs`, тестовый
   конструктор `App` с `FakeEngine`; затем собрать app-тесты.
2. Обновить snapshot после просмотра и добавить app-level kittest-покрытие навигации и каталогов.
3. Убрать пересоздание scheduler на каждое изменение бюджета: обновлять pools у живого
   scheduler либо останавливать и дожидаться старого host до `recover`, с дебаунсом.
4. Сделать сохранение настроек независимым от keychain; секреты грузить один раз в агенте.
5. Ленивый запуск engine/queue, эффективный язык интерфейса для агента, `Job`/`LatestWorker`
   вместо ad-hoc потоков.
6. Переносимая build-конфигурация (без VS-специфичного генератора в общем конфиге),
   документированные Vulkan SDK/CMake, чистка неиспользуемых зависимостей и профилей.
7. Синхронизировать README, `architecture.md`, `constraints.md` и этот документ.

## Выполнено 27.10.26

- Сборка app: `player` стал `pub(crate)`, устранены E0499 в `catalogs` и E0507 в `theme`,
  тестовый конструктор `App` получил `AgentController` на `FakeEngine`, `page_title`
  покрывает страницы агента.
- Жизненный цикл: `Scheduler::set_pools` обновляет pools живого scheduler (новый тест в
  `queue/tests/queue.rs`); scheduler больше не пересоздаётся при изменении бюджета.
  Добавлен `features/agent::AgentController`; `pages/agent` содержит только UI, layout
  каталогов перенесён в `catalogs::screen`, `tab_strip` перенесён в `pages/agent`.
- Движок опрашивается только при открытом разделе Agent; host очереди запускается при открытом
  разделе или незавершённых задачах (`run::has_unfinished`).
- Секреты провайдеров перенесены из `core` в `agent/secrets`; `Settings::load` больше не ходит
  в keychain, `SettingsWriter` пишет файл независимо и синхронизирует ключи best-effort.
- Язык агента берётся из `i18n::current()`.
- UI-фоновые операции переведены на общие механизмы: проверки провайдеров, статус моделей,
  inspect, загрузка и удаление моделей, поиск в каталогах и диалоге используют
  `jobs::Job`/`latest_worker::LatestWorker` вместо собственных потоков с `Receiver`.
- Build: убраны `CMAKE_GENERATOR`, `CMAKE_TOOLCHAIN_FILE` и `cmake/static-crt.cmake`;
  `LLAMA_STATIC_CRT=1` достаточно для llama.cpp; `Directory.Build.targets` ограничен проектами
  llama.cpp; `env.ps1` проверяет CMake и Vulkan SDK; добавлены overrides dev/test-fast.
- README, `architecture.md`, `constraints.md` обновлены; добавлены kittest-тесты навигации
  агента и snapshot `agent-catalogs-wide`.
- Шаблоны `crates/agent/prompts/**/*.j2` и snapshots `crates/agent/src/snapshots/*.snap`
  нормализованы в LF: после checkout они оказывались в CRLF, из-за чего prompt-тесты
  сравнивали `\r\n` с `\n` и падали на Windows.

Итог проверки — в разделе «Проверка».

## Проверка

- `cargo fmt --all -- --check` — чисто.
- `cargo check --workspace --lib --bins --profile dev-check` — чисто.
- `cargo build -p mediaa --profile dev-check` — линкуется.
- `cargo clippy --workspace --all-targets -- -D warnings` — чисто.
- `cargo test -p mediaa-core -p mediaa-queue` — 121 и 21+14+4+24 passed (очередь включает
  новый тест изменения pools; 1 ignored — OS keychain).
- `cargo test -p mediaa-agent --lib` — 211 passed, 14 ignored (только после нормализации
  line endings шаблонов `.j2`).
- `cargo test -p mediaa-engine --lib` — 59 passed, 8 ignored; `mediaa-eval` — 26 passed.
- `cargo test -p mediaa --lib -- --test-threads=1 --skip ui_regression_snapshot` — 229 passed;
  ещё 3 теста (два overlay и notice fade) нестабильны при занятом GPU/нагрузке и по отдельности
  проходят. При прогоне без конкуренции за GPU весь набор проходил (232 passed).
- 18 page-snapshot тестов проходят по отдельности (`WGPU_BACKEND=dx12`); baseline обновлены,
  новый `agent-catalogs-wide` просмотрен. Четыре overlay-snapshot проверены группой вместе с
  русской локалью — проходят. Полный UI-прогон в одном процессе на этой машине периодически
  падает по `RequestDeviceError(Device Lost)` — ограничение окружения.
- Тестовые данные UI используют `C:\Users\Test\Documents\MediaA` для snapshot-стабильности;
  страницы агента — временный каталог, чтобы не писать в чужую папку.

## Материальные риски

- Linux/macOS-сборка не проверена: нет окружения; конфигурация больше не задаёт
  `CMAKE_GENERATOR`/`CMAKE_TOOLCHAIN_FILE`, а Windows-сборка проверена на чистом каталоге
  llama.cpp.
- Нативные зависимости (`llama.cpp` + Vulkan, `sherpa-onnx`) требуют CMake и Vulkan SDK;
  без `VULKAN_SDK` сборка падает в build.rs.
- Полный `cargo test --workspace` в одном процессе на этой машине нестабилен из-за
  конкурирующего GPU-процесса; функциональные прогоны выполнены по группам.
