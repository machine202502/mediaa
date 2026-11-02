# ST-12: Frontend-owned state и клиентские предпочтения

## Решение

Frontend владеет клиентским состоянием и может сохранять его на диск.
Backend про это состояние не знает, через sync оно не идёт.

Это не «только эфемерные вкладки». Это полноценное состояние клиента:

- громкость просмотрщика;
- последняя страница/вкладка;
- размеры и положение панелей;
- сортировка/фильтр библиотеки;
- последний открытый файл;
- тема;
- локальные подсказки.

## Текущая проблема

`viewer_volume` сейчас находится в `mediaa_core::Settings`:

```rust
pub struct Settings {
    pub viewer_volume: f32,
    ...
}
```

Backend сохраняет его в `.mediaa.toml` и отдаёт frontend-у. По новой модели
громкость нужна только frontend-плееру, backend не должен её знать.

## Целевая модель

```rust
#[derive(Serialize, Deserialize)]
pub struct ClientPreferences {
    pub schema: u32,
    pub viewer_volume: f32,
    pub last_page: Page,
    pub library_sort: SortMode,
    pub library_filter: FilterMode,
    pub theme: ThemePreference,
    pub last_file: Option<PathBuf>,
}
```

Хранение:

- файл `ui-state.toml` в data dir frontend-а;
- атомарная запись temp + rename;
- битый файл -> defaults + warning;
- неизвестные поля игнорируются;
- версия схемы и миграции.

## Что не входит

- `media_dir`, `convert_dir`, recording settings — backend, потому что
  влияют на файлы и запись;
- `close_to_tray` — зависит от того, кто завершает процесс; решается в
  ST-1;
- `tray_progress` — влияет на backend tray;
- agent connections/api keys — backend/keychain.

## Миграция громкости

1. Добавить `ClientPreferences::load/save`.
2. При старте frontend читает `ui-state.toml`.
3. Если файла нет, взять `Settings::viewer_volume` как fallback.
4. Записать в `ui-state.toml`.
5. Убрать `viewer_volume` из `Settings` (или оставить legacy на один
   релиз с пометкой).
6. Backend больше не сериализует громкость.

## Тесты без Windows

1. `volume_survives_restart`
2. `missing_file_uses_defaults`
3. `corrupt_file_uses_defaults_and_keeps_old_file`
4. `old_schema_is_migrated`
5. `unknown_fields_are_ignored`
6. `atomic_write_leaves_no_temp_file`
7. `legacy_settings_volume_is_migrated_once`
8. `backend_settings_no_longer_contain_viewer_volume`

## Критерии готовности

- Громкость не проходит через backend.
- `ui-state.toml` — единственный источник клиентских предпочтений.
- Backend не импортирует `ClientPreferences`.
- Все тесты выше зелёные без Windows.
