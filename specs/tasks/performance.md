# Library и WebP performance checks

## Статус

Базовые benchmark и решения по H1, H2 и H4 зафиксированы 26.10.2026.

## Инструменты

Инструкции для агентов по Windows-скриптам: [scripts/windows/AGENTS.md](../../scripts/windows/AGENTS.md)
(`build`, `run`, `test`, `agent`); короткий список для человека — `README.md` там же.

```text
crates/storage/examples/library_bench.rs
crates/media/examples/animation_bench.rs
crates/media/examples/encode_bench.rs
crates/media/examples/preview_bench.rs
crates/media/examples/proxy_bench.rs
crates/platform/examples/capture_bench.rs
crates/platform/examples/webp_matrix.rs
scripts/windows/checks/measure-memory.ps1
scripts/windows/checks/measure-picker.ps1
scripts/windows/checks/run-background.ps1
```

`measure-picker.ps1` меряет холодный старт выбора области: запускает инстанс и опрашивает
появление видимых окон `MediaA picker mask` и `MediaA area picker`, публикует min/p50/p95/max
для обоих. С `-WarmDaemon` демон поднимается один раз, поэтому замеряется именно старт UI, а
не запуск демона. По умолчанию работает в фоновом режиме: окна за экраном, без активации и без
маски; `-Foreground` возвращает обычное поведение.

`run-background.ps1 -Exe <path> [-Arguments @('--daemon')] [-Wait]` запускает любой бинарник
MediaA с `MEDIAA_DEV_BACKGROUND=1`: главное окно скрыто, окна за экраном, фокус возвращается
прежнему окну. Агенты используют его для всех оконных проверок, чтобы ничего не всплывало.

## Аудит памяти и CPU приложения

`scripts/windows/checks/measure-memory.ps1` запускает изолированный экземпляр (dev-библиотека и
instance id) в заданных состояниях и раз в секунду снимает private bytes, working set, CPU
(в процентах одного ядра) и число потоков. Состояния задаются теми же аргументами, что у
приложения: `--page=…`, `--tray`, `--area`, `--daemon`. Результат — CSV со всеми сэмплами, Markdown-таблица
(p50/p95/max) и JSON-снимок в `target/perf`: `audit-<stamp>.json` и `audit-latest.json`.

```powershell
.\scripts\windows\checks\measure-memory.ps1 -States daemon,tray
.\scripts\windows\checks\measure-memory.ps1 -States about,library,tray
.\scripts\windows\checks\measure-memory.ps1 -Env @{ WGPU_BACKEND = 'gl' }
```

- Изоляция работает только в dev-сборке: release игнорирует `MEDIAA_DEV_LIBRARY` и
  `MEDIAA_INSTANCE_ID`. Release-аудит требует отдельного профиля или отдельного решения.
- Цифры зависят от машины, драйвера и экрана, поэтому эталонного замера в репозитории нет:
  каждый сравнивает свои снимки сам. `-Compare <json>` и `-ThresholdPercent` остаются
  необязательным удобством для сравнения двух прогонов на одной машине.
- `-Env` задаёт переменные окружения приложения, например другой бэкенд wgpu для сравнения.
- Состояния и формат отчёта не зависят от ОС; различается только чтение метрик. Для Linux/macOS
  скрипта нет: эти платформы не поддерживаются и не проверяются обычным прогоном.

## Запись в демоне без UI

`scripts/windows/checks/record-smoke.ps1` запускает только `--daemon` с
`MEDIAA_DEV_RECORD=<сек>`: демон сам пишет регион 320×200, кладёт файл в изолированную
dev-библиотеку и пишет замер в `target/perf/record-smoke-<stamp>.md`. Окна не появляются.

```powershell
.\scripts\windows\checks\record-smoke.ps1 -Seconds 3
.\scripts\windows\checks\record-smoke.ps1 -Seconds 5 -Keep
```

Замер на этой машине (debug, GIF, 3 с): private p50 **25.1 MB**, p95/max **63.5 MB**, working
set max **68.2 MB**, CPU **547 ms** за запись. Пик — кодирование GIF; UI-процесс не участвует.

## Library index

Локальный Windows/NTFS synthetic benchmark:

| Библиотека | scan p50 | scan p95 | apply_scan |
|---|---:|---:|---:|
| 10k | 18,4 ms | 19,5 ms | 5,4 ms |
| 50k | 157,3 ms | 186,7 ms | 54,3 ms |

Watcher создал 2 000 файлов без overflow и пропусков. Burst из 50 удалений и 50 добавлений на
50k вызвал overflow; fallback scan+apply занял примерно 152/63 ms и восстановил индекс без
stale/missing записей.

Решение: periodic rescan сохраняется как межплатформенная страховка. Следующая оптимизация —
уменьшить UI hitch от `apply_scan`, а не удалять recovery.

## MemoryBudget

Проверено 288 комбинаций resolution, FPS, quality, duration и budget:

- 82 конфигурации получали сегмент сверх бюджета после повторного округления;
- 0 конфигураций имели aggregate-only превышение поверх `worker_limit`;
- 23 106 сегментов имели различие между планируемым и scheduler frame count.

Решение: per-segment budget guard необходим. CAS aggregate reservation можно заменить точным
preflight-расчётом только после общего расчёта rounding/split и отдельного RSS A/B.

## WebP hypotheses

- Atlas оставлен.
- Live WebP оставлен.
- SQLite оставлен.
- Permanent normalize/validate/panic не повторяются.
- Budget resource и unknown failures сохраняют bounded retry.
- Clipboard timeout/restart и SettingsWriter не упрощаются без fault-injection.

## Правило новых замеров

Сравнивать одинаковые input fixtures, отдельно cold/warm, минимум 10 повторов, p50/p95/max,
RSS и корректность результата. Микротест не заменяет end-to-end пользовательский сценарий.
