# ARC: единая структура frontend и backend

## Цель

Заменить исторические имена слоёв на явные и привести оба крейта к одной
feature-first структуре, не меняя поведение.

## Решение

Frontend:

```text
ui/         отрисовка и локальные виджеты
app/        composition root: App, окно, event loop, выполнение Intent
features/   состояние фич и use cases
infra/      ОС и транспортные швы
```

Backend:

```text
daemon/     composition root: процесс, event loop, IPC dispatch
features/   предметные сервисы: agent, library, preview, capture,
            clipboard, recording, conversion
infra/      ipc, tray, hotkeys, settings_file
```

Правила границ:

- `features` не импортирует `ui` и `app`;
- `infra` не импортирует `ui` и `features`;
- `ui` не импортирует `app` и не вызывает ОС/fs/threads напрямую;
- `daemon` и `app` — единственные composition roots.

`boundaries` (бывший `layer_check`) проверяет внутренние границы frontend.
Разделение на frontend/backend проверяет `mediaa-arch`, но не заменяет
внутренние границы.

## Разбиение daemon

```text
daemon/mod.rs       Daemon, run, event loop
daemon/dispatch.rs  IPC dispatch и event handling
daemon/agent.rs     агентские команды и poll
daemon/services.rs  library/preview/conversion/recording/recovery
daemon/notices.rs   NoticeRing
daemon/launch.rs    launcher/daemon/ui процесс
```

## Проверка

```powershell
cargo check --workspace --all-targets
cargo test -p mediaa-frontend --lib
cargo test -p mediaa-backend --lib
cargo test -p mediaa-arch
cargo clippy --workspace --all-targets
cargo fmt --check
```

## Результат

- frontend: `app/`, `features/`, `infra/`, `ui/`, `boundaries`, `protocol`, `session`.
- backend: `daemon/{mod,dispatch,agent,services,notices,launch}`, `features/*`, `infra/*`.
- удалён мёртвый `backend/src/capture.rs`.
- `cargo test -p mediaa-frontend --lib` — 334 passed.
- `cargo test -p mediaa-backend --lib` — 52 passed (включая `backend_modules_respect_architecture_boundaries`).
- `cargo test -p mediaa-arch` — 6 passed.
- `cargo check --workspace --all-targets` — зелёный.
- clippy: только внешний `mediaa-gif`.

