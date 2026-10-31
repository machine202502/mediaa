# Покрытие тестами превью и кэшей

## Сценарии

- Настройки превью включены по умолчанию и сохраняются/читаются
  (`core::settings::preview_options_are_on_by_default`).
- UI настроек: выключение «Анимированные превью» удаляет только `hover-*.mp4`, выключение
  «Видео-превью для полного просмотра» — только остальные preview-файлы
  (`app::ui_tests::preview_options_clear_only_their_own_cache_when_disabled`).
- Категорийная очистка кэша (`preview_cache::tests::clearing_previews_can_target_animated_or_full_cache_independently`).
- Hover-превью хранит полную длительность и не показывает спиннер до готовности прокси
  (`hover_preview::tests::missing_proxy_keeps_full_duration_without_a_spinner`).
- Hover-превью не срезает длительность
  (`hover_preview::tests::metadata_arrival_*`, `hovering_again_resumes_where_it_was_left`).
- Backpressure просмотрщика: второй кадр ждёт освобождения очереди, не выбрасывается
  (`player::tests::preview_sender_waits_for_room_instead_of_dropping_frames`).
- Фоновая очистка: старые preview и старые atlas-страницы удаляются, свежие остаются
  (`cache_maintenance::tests::stale_previews_and_atlas_pages_are_removed`).
- Очистка atlas-страницы удаляет её строки из БД
  (`thumb_cache::tests::pruning_an_atlas_page_removes_its_database_rows`).
- Снапшоты страницы настроек обновлены под новую секцию «Превью».

## Прогон

- app: **263**.
- core: **138**.
- media lib: **46**, media pipeline: **30**.
- storage: **16**.
- `cargo clippy --workspace --all-targets`, `cargo fmt --all`.
