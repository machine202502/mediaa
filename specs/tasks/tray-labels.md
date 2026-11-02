# BUG: подписи трея «backend message»

## Симптом

В меню трея все пункты назывались `backend message`.

## Причина

После разделения на frontend/backend трей остался в backend. В backend был
локальный макрос-заглушка:

```rust
macro_rules! t {
    ($($tt:tt)*) => { std::borrow::Cow::Borrowed("backend message") };
}
```

Он перекрывал `rust_i18n::t!`, а locale-файлы жили только во frontend.
`crate::i18n::current()` всегда возвращал `"en"`.

## Решение

- backend загружает те же locale-файлы: `rust_i18n::i18n!("../frontend/locales")`;
- удалён `crates/backend/src/macros.rs`;
- `crate::i18n` теперь умеет `apply(settings.language)` и учитывает
  системную локаль через `sys-locale`;
- `daemon::run` применяет язык до создания трея;
- `apply_settings` применяет язык и вызывает `Tray::refresh`, чтобы подписи
  обновились без перезапуска;
- `Tray::refresh` сбрасывает кэш `applied` и заново выставляет подписи.

## Проверка

`crates/backend/src/i18n.rs::tests::tray_labels_are_localized`:

```text
ru: Открыть MediaA
en: Open MediaA
```

## Файлы

- `crates/backend/src/i18n.rs`
- `crates/backend/src/lib.rs`
- `crates/backend/src/infra/tray.rs`
- `crates/backend/src/daemon/mod.rs`
- `crates/backend/Cargo.toml`
- удалён `crates/backend/src/macros.rs`
