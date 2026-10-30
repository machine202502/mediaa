# Запрет неиспользуемого кода

## Настройка

В корневом `Cargo.toml`:

```toml
[workspace.lints.rust]
unused = "deny"
dead_code = "deny"
```

Все crate-манифесты наследуют:

```toml
[lints]
workspace = true
```

Проверка: `cargo check --workspace --all-targets` и `cargo clippy
--workspace --all-targets`.

## Результат проверки

Компилятор не нашёл неиспользуемых элементов, которые можно удалить. Остались только
осознанные исключения:

- `crates/app/build.rs` — общий модуль иконок для build-скрипта, часть функций нужна только
  приложению;
- `crates/platform/src/cursor_overlay.rs` — поле читается только на Windows.

Новых `allow(dead_code)` не добавлялось.
