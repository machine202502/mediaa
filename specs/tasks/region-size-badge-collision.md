# Перекрытие size badge и активного текста

## Статус

В работе с 26.10.26 07:09.

## Цель

Не позволять плашке с размером selection перекрывать активный text editor, если текст перетащен
через верхнюю или нижнюю границу области.

## Контекст

Плашка всегда сначала размещалась над selection. При clamp к верхнему краю экрана она попадала на
active text frame. Это выявлено snapshot-тестом `region-editor-active-text`.

## Решение

- Рассматривать несколько позиций вокруг selection и внутри его свободных углов.
- Всегда clamp-ить badge в границы display.
- Минимизировать площадь пересечения с active text frame.
- Сохранить существующее поведение при отсутствии активного текста.

## Проверка

- Geometry unit test проверяет отсутствие пересечения с active text frame.
- Active text snapshot обновляется и просматривается глазами.
- Existing overlay/app UI tests, Clippy и format.

## Результат

- badge перебирает восемь позиций вокруг и внутри selection;
- каждая позиция clamp-ится в границы display;
- выбирается позиция с минимальным пересечением с active text frame;
- active-text snapshot подтверждает, что badge переехал в свободный угол.

## Выполненные проверки

- targeted geometry test — passed;
- active-text snapshot обновлён и визуально проверен;
- `cargo test --workspace` — 135 passed в app crate;
- `cargo test -p mediaa --lib app::ui_tests` — 42 passed;
- `cargo test -p mediaa --lib overlay::tests::` — 12 passed;
- native overlay smoke — passed;
- Clippy и format — успешно.
