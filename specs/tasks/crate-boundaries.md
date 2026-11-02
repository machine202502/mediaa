# Границы между крейтами

## Задача

`cargo` сам не даст импортировать незадекларированную зависимость и не пропустит цикл. Но он
разрешает **любую объявленную** зависимость, поэтому направление между крейтами держится только
соглашением. Тест `mediaa-arch` фиксирует это соглашение и падает, когда оно меняется.

Тест проверяет:

1. каждый член workspace описан правилом (новый крейт без правила = падение);
2. обычные и dev-зависимости каждого крейта совпадают с allowlist;
3. ни один крейт не зависит от `mediaa` (приложением);
4. внутренних build-зависимостей нет;
5. feature `testing` внутренних зависимостей включён только в dev;
6. `mediaa_engine::testing` встречается только в тестовом коде.

## Где живёт

```text
crates/arch/
  Cargo.toml     # mediaa-arch, publish = false, зависит только от toml
  src/lib.rs     # CrateRule, RULES, чтение Cargo.toml, сравнение
  tests/deps.rs  # четыре теста
```

Запуск: `cargo test -p mediaa-arch` (входит и в `cargo test --workspace`).

## Правила

| Крейт | Обычные зависимости | Dev-зависимости |
|---|---|---|
| `mediaa-core` | — | — |
| `mediaa-storage` | `core` | — |
| `mediaa-render` | `core` | — |
| `mediaa-ffmpeg` | `core` | — |
| `mediaa-webp` | `core`, `ffmpeg` | — |
| `mediaa-gif` | `core`, `ffmpeg` | — |
| `mediaa-media` | `core`, `ffmpeg`, `webp`, `gif` | `storage`, `render` |
| `mediaa-platform` | `core` | `media`, `storage` |
| `mediaa-engine` | `core` | — |
| `mediaa-queue` | — | — |
| `mediaa-agent` | `core`, `engine`, `media`, `queue` | `engine` (с `testing`) |
| `mediaa-eval` | `core`, `engine`, `agent`, `queue` | — |
| `mediaa` (app) | `core`, `storage`, `render`, `media`, `ffmpeg`, `platform`, `engine`, `agent`, `queue` | `engine` (с `testing`) |
| `mediaa-arch` | — | — |

Правила — это `mediaa_arch::RULES` в `crates/arch/src/lib.rs`. Таблица в документе должна с ними
совпадать.

## Как менять

- **Новая зависимость:** сначала решить, законна ли она. Если да — добавить в `RULES` и в таблицу
  выше одним изменением; если нет — тест укажет, что именно лишнее.
- **Новый крейт:** добавить `CrateRule` с его настоящими зависимостями; тест
  `workspace_members_are_described` этого потребует.
- **Удалённая зависимость/крейт:** правило тоже удалить — тест падает на «пропала зависимость»,
  чтобы правила не гнили.

Сообщение об ошибке содержит крейт, лишнюю зависимость, разрешённый список и что делать.

## Feature-гигиена

`tests/features.rs` продолжает проверку и ловит попадание тестовых заглушек в production:

- feature `testing` у внутренних зависимостей разрешён только в `[dev-dependencies]`
  (обычные, build- и target-зависимости — нарушение);
- `mediaa_engine::testing` встречается только в тестовом коде: `#[cfg(test)]`-блоки вырезаются,
  файлы `tests.rs`/`test_support.rs`/`ui_tests.rs` и каталоги `tests/` считаются тестовыми.

Обе проверки падают и на «правило устарело», если feature или модуль больше не используются —
так правило не гниёт.

## Что не проверяется

- **Внутрикрейтовые слои frontend.** Это делает `crates/frontend/src/boundaries.rs`:
  `features` не видит `ui`/`app`/egui, `infra` не видит `ui`/`features`, `ui` не вызывает
  ОС/fs/threads. Backend использует `daemon`/`features`/`infra`, но отдельной проверки
  внутрикрейтовых слоёв там пока нет.
- **Эффекты** (`unsafe`, `std::process`, сеть). Решено не запрещать: `unsafe` разрешён везде,
  остальное — по мере необходимости.
- Поведение, производительность и рантайм-инварианты.
