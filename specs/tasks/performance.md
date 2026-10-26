# Library и WebP performance checks

## Статус

Базовые benchmark и решения по H1, H2 и H4 зафиксированы 26.10.2026.

## Инструменты

```text
crates/storage/examples/library_bench.rs
crates/webp-pipeline/examples/memory_probe.rs
crates/media/examples/animation_bench.rs
```

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
