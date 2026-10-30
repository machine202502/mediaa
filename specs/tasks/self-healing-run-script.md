# Самовосстанавливающийся run.ps1

## Решение

- `scripts/windows/run.ps1` при падении сборки один раз сбрасывает кэш `ffmpeg-sys-next`
  (`cargo clean -p ffmpeg-sys-next`, удаление `target\<profile>\build\ffmpeg-sys-next-*` и
  `.fingerprint\ffmpeg-sys-next-*`), заново выставляет `FFMPEG_DIR` и повторяет сборку.
- Повтор идёт с `-vv` и пишет `target\build-failure.log`; если сборка снова падает, скрипт
  печатает последние строки с ошибками/паникой и путь к логу.
- Перед первой сборкой печатаются `FFMPEG_DIR`, `LIBCLANG_PATH` и `VCPKG_ROOT`, чтобы причина
  была видна без ручной диагностики.
- До запуска `env.ps1` скрипт снимает блокировку со всех `*.ps1` рядом с собой, поэтому
  предупреждение о запуске скачанных сценариев больше не повторяется.
- Добавлен `scripts/windows/run.cmd`: запускает PowerShell с `-ExecutionPolicy Bypass`, чтобы
  не отвечать на запрос политики выполнения.

## Файлы

- `scripts/windows/run.ps1`
- `scripts/windows/run.cmd`

## Проверка

- Синтаксис `run.ps1` проверен парсером PowerShell.
- Сборка приложения и тесты не затронуты.
