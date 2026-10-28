# Windows-сборка после переноса агента

## Цель

`.\scripts\windows\run.ps1` снова собирает и запускает MediaA.

После `AGENT-EXT-1` сборка больше не требует CMake, Vulkan SDK и static CRT toolchain:
llama.cpp из дерева зависимостей убран. Ниже — расследование сборки, в которой он ещё был.

## Контекст

`feature/agent-temp` собиралась, потому что `.cargo/config.toml` задавал
`CMAKE_GENERATOR = Visual Studio 18 2026` и `CMAKE_TOOLCHAIN_FILE = cmake/static-crt.cmake`.
При переносе эти настройки убрали из общего конфига, а файл toolchain удалили из рабочего дерева.
Кэш CMake в `target/` продолжал подключать удалённый файл.

Общий конфиг по-прежнему не должен задавать генератор: он ломает Linux, macOS и другую версию
Visual Studio. Переменные выставляет только `scripts/windows/env.ps1`. Генератор выбирается по
установленной VS. Toolchain задаёт статический CRT, потому что явный генератор VS не переносит
`/MT` в флаги конфигурации.

Профиль `dev` собирается без debug info. `cc` 1.8 передаёт MSVC `-Z7`, а компилятор 14.51 на длинной
командной строке отвечает D8050 и кодом 2. Без `-Z7` те же файлы компилируются.

Сборка падает, если песочница OpenAI Codex для Windows (`CodexSandboxService`) пометила
репозиторий меткой целостности Low (`icacls .` показывает `Mandatory Label\Low Mandatory
Level:(OI)(CI)(NW)`). Метка наследуется в `target`, в том числе на `build-script-build.exe`.
Windows запускает процесс с уровнем не выше метки его exe, поэтому build-скрипты и запущенные
ими `cl.exe`, `lib.exe` и MSBuild работают на Low. Процессу Low запрещено писать в `%TEMP%`
уровня Medium:
- `lib.exe` (`lzma-sys`, `aws-lc-sys`): LNK1104 на `%TEMP%\lnk{GUID}.tmp`;
- MSBuild (`llama-cpp-sys-2`): `UnauthorizedAccessException` на `%TEMP%\MSBuildTemp<GUID>`;
- `cl.exe` с `-Z7`: код 2 без сообщения.

Обход с `TEMP` в `target\tmp` работал, потому что этот каталог тоже имел метку Low. Сборка вне
репозитория (`%TEMP%`, `C:\shared-data`) проходила, потому что там метки нет.

Исправление 28.10.26, без обходов в `env.ps1`:

```powershell
icacls <repo> /reset /T /C /Q
icacls <repo> /setintegritylevel "(OI)(CI)M" /C /Q
```

Первая команда снимает DACL-записи Codex (`Everyone: DENY DeleteSubdirectoriesAndFiles`,
`CodexSandboxUsers` и неизвестные SID). Сами по себе они сборку не ломали. Вторая возвращает
уровень Medium и наследует его вниз. Сохранённые права корня до правки лежат в
`C:\shared-data\gifcap2-acl-backup.txt`. После этого `cargo build -p mediaa` с системным
`%TEMP%` проходит. Codex снова ставит метку, когда работает в этом каталоге в режиме песочницы.
Остановка процесса Codex метку не снимает.

Если сборка оборвалась на шаге архивации, в `out` остаётся частичный `.a`, и повторный запуск
даёт LNK1114. Такой `.a` удаляют.

## Проверка

`powershell -File .\scripts\windows\env.ps1` не запускают отдельно: скрипт только готовит окружение.
Сборка: тот же env, затем `cargo build -p mediaa`. Запуск — отдельный процесс с
`MEDIAA_DEV_LIBRARY` и `MEDIAA_INSTANCE_ID`, не пользовательский экземпляр.
