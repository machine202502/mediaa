# Конвертация из библиотеки без перехода на вкладку

## Цель

Кнопка «Конвертировать…» на карточке библиотеки и пункт «Конвертировать…» в меню выделенной
группы добавляют файлы в конвертацию, но больше не переносят пользователя на вкладку
«Конвертировать». Пользователь остаётся в библиотеке и сам решает, когда открыть вкладку.

## Причина

`shell/actions.rs::open_in_convert_many` после `conversion.add_inputs(paths)` выставлял
`self.page = Page::Convert`. Через этот метод шли оба входа из библиотеки
(`ui/pages/library/actions.rs`: `LibraryAction::Convert` и `ConvertMany`), поэтому любое добавление
файла уводило со страницы.

## Решение

- `open_in_convert_many` только наполняет очередь конвертации; навигация не меняется;
- проверка занятости конвертации и предупреждение `convert.busy` сохранены;
- неиспользуемый одиночный `App::open_in_convert` удалён: остался единственный вход через
  `Intent::OpenInConvert`.

## Затронутые файлы

- `crates/app/src/shell/actions.rs` — метод `open_in_convert_many`;
- `crates/app/src/shell/ui_tests.rs` — тесты карточки и выделенной группы.

## Проверка

- `cargo test -p mediaa --lib -- converting_` — после клика по карточке и после конвертации
  выделения страница остаётся `Page::Library`, файлы попадают в `conversion.batch`;
- `cargo test -p mediaa --lib`;
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`.

## Материальные риски

- Клик по «Конвертировать…» не открывает вкладку конвертации: файлы попадают в очередь, а обратную
  связь даёт уведомление с именами файлов
  ([notifications-default-and-convert-queue.md](notifications-default-and-convert-queue.md)).
