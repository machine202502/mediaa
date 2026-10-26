# Удаление дублей из CLAUDE.md

## Статус

Завершено 26.10.2026.

## Цель

Оставить `CLAUDE.md` только точкой входа к `specs/`. Общие принципы должны находиться в
`specs/principles.md`, обязательные правила — в `specs/rules.md`.

## Решение

- Удалить из `CLAUDE.md` decision model, workflow, Git, code rules, отчётность и описание skills.
- Оставить только порядок загрузки контекста.
- Перенести точные исключения комментариев, Clippy и project skills в `specs/rules.md`.
- Перенести статус unreleased development в `specs/architecture.md`.
- Не создавать новый источник правил.

## Проверка

- `CLAUDE.md` содержит только навигацию и порядок чтения;
- `rg` не находит смысловых копий правил между `CLAUDE.md` и `specs/`;
- все ссылки валидны;
- `git diff --check` чистый.
