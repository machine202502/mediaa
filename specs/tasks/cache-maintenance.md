# Периодическая очистка кэшей

## Что добавлено

Пока приложение запущено, раз в 30 минут фоновый `cache-sweep` проверяет:

- `<медиа>/.cache/previews/*.mp4` — обычные и hover-прокси;
- `<медиа>/.cache/thumbs-*.jpg` — атласы миниатюр.

Файлы старше **7 дней** удаляются. Для атласов также удаляются записи из `thumbs`-таблицы,
относящиеся к удалённым страницам.

## Время жизни

Возраст определяется по времени изменения кэш-файла. Когда пользователь снова открывает старый
ролик, прокси/миниатюра генерируются заново, и у кэша появляется новое время жизни. Старые
файлы, которые больше не открывали, постепенно вычищаются.

## Файлы

- `crates/media/src/preview_cache.rs` — `prune_previews`.
- `crates/storage/src/thumb_cache.rs` — `prune_pages` + удаление записей БД.
- `crates/app/src/features/cache_maintenance.rs` — `sweep` и `SweepReport`.
- `crates/app/src/app.rs` — запуск `cache-sweep` раз в 30 минут.

## Тесты

- `cache_maintenance::tests::stale_previews_and_atlas_pages_are_removed` — старые превью и
  страница атласа удаляются, свежие остаются.
- `thumb_cache::tests::stale_atlas_pages_are_pruned`.
- Полный прогон: app **259**, core **138**, media pipeline **30**, storage **5**
  thumb_cache-тестов; `clippy` чистый.
