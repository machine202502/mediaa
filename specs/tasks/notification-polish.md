# Полировка уведомлений

## Статус

Завершено 26.10.26 05:38.

## Цель

- добавить плавное появление и исчезновение карточек;
- гарантировать автоматическое исчезновение любого уведомления, включая открытую карточку;
- запретить действиям внутри уведомления создавать другие уведомления;
- выровнять icon по центру header row с заголовком.

## Найденные причины

- `Notices::expire` сохраняет expanded notice независимо от `expires_at`;
- `show` не планирует repaint к моменту истечения;
- CopyLog/CopyFiles/CopyFile используют `Purpose::Toast`, а clipboard event создаёт success/error
  notice;
- RevealFile использует обычный desktop path, который сообщает ошибку через notice;
- icon и summary находятся в одной horizontal row, но без общей фиксированной высоты header.

## Решение

- expired notice сначала входит в короткую closing animation, затем удаляется;
- appearance animation использует fade, closing использует fade и удаление после завершения;
- repaint планируется на ближайший expiry или следующий animation frame;
- expanded влияет только на размер карточки, не на lifetime;
- hover приостанавливает оставшийся lifetime и продолжает его после выхода указателя;
- clipboard purpose `Silent` не создаёт UI notice;
- immediate clipboard submit errors логируются без нового notice;
- desktop reveal из notice подавляет error notice;
- header row имеет фиксированную высоту и centered icon/title layout.

## Проверка

- unit tests для appearance/closing progress;
- UI test: expanded уведомление истекает и исчезает;
- app test: CopyLog использует clipboard event без добавления notice;
- app test: silent desktop error не добавляет notice;
- UI/geometry и snapshots не должны ухудшить layout;
- targeted tests, затем relevant app UI tests и Clippy.

## Риски

Closure animation должна удалять элемент только после fade, иначе исчезновение снова будет
мгновенным. Silent clipboard скрывает ошибку copy от пользователя; это прямое требование
«уведомление не порождает другое уведомление». Ошибка остаётся в логе.

## Результат

- full workspace suite: 119 app tests и остальные workspace tests прошли;
- UI suite: 35 tests прошли;
- tested fade in/out, auto-expire expanded notice, hover pause/resume, silent copy, silent desktop
  error и выравнивание icon/title;
- collapsed и expanded cards визуально проверены по snapshots;
- Clippy с `-D warnings` и format check прошли.
