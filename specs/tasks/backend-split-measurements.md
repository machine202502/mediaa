# SPLIT2: замеры процесса

Дата: 03.11.26. Сборка: `dev` (`cargo build -p mediaa`), Windows.
Сценарий: `mediaa.exe --page=library`, ожидание 6 секунд, затем оба процесса.

| Роль | PID | Окно | WorkingSet |
|---|---:|---|---:|
| backend (`--backend`) | 1660 | нет | 16.9 MB |
| UI (`--ui`) | 24656 | MediaA | 550.0 MB |

## Что подтверждено

- backend-процесс без окна;
- UI-процесс с окном;
- оба живут одновременно;
- backend слушает `\\.\pipe\mediaa-default` (см. smoke-лог).

## Ограничения замера

- debug-сборка, не release;
- UI держит egui/wgpu-контекст, поэтому 550 MB не показатель release;
- замер не включает реальную запись/превью-нагрузку;
- цифры нельзя сравнивать с release без повторного прогона.

## Следующий замер

1. Release-сборка.
2. Простой UI.
3. UI + библиотека.
4. Запись 30 секунд.
5. Preview-очередь на 100+ элементов.
6. RSS/CPU/время старта backend и UI отдельно.

## Release-замер

Дата: 03.11.26. Сборка: `cargo build --release -p mediaa`.
Сценарий: `mediaa.exe --page=library`, 6 секунд.

| Роль | PID | Окно | WorkingSet | CPU |
|---|---:|---|---:|---:|
| backend (`--backend`) | 35464 | нет | 15.7 MB | 0.06 s |
| UI (`--ui`) | 39764 | MediaA | 503.0 MB | 5.69 s |

UI остаётся тяжёлым из-за egui/wgpu; backend-процесс — около 16 MB.
## Release-замер после finalize-split

Дата: 02.11.26. Сборка: `cargo build -p mediaa --release`.
`target/release/mediaa.exe` — 78 961 152 байт (75.3 MiB), один exe.
Сценарий: `mediaa.exe --backend`, затем `mediaa.exe --ui --page=library`, 8 секунд.

| Роль | PID | Окно | WorkingSet | CPU |
|---|---:|---|---:|---:|
| backend (`--backend`) | 40432 | нет | 18.4 MB | 0.03 s |
| UI (`--ui`) | 9680 | MediaA 3204×1919 | 527.9 MB | 7.06 s |

Проверено: оба процесса живы одновременно, UI открыл «Библиотеку», backend не имеет окна.
Release exe содержит и launcher, и `--backend`, и `--ui`; отдельных бинарников нет.
UI остаётся тяжёлым из-за egui/wgpu. Замер не включает запись, preview-очередь и conversion.

Повторный smoke после P4 (library remove/preview backend), 02.11.26:
`mediaa.exe` 78 944 768 байт; backend 18.4 MB; UI 527.5 MB, окно «Библиотека».

Финальный smoke (после удаления всех dead_code-allow), 02.11.26:
`mediaa.exe` 78 950 912 байт; backend 18.1 MB; UI 533.9 MB, окно «Библиотека».
