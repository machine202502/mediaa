# Скилл видео-тестов

## Что удалено

Удалены старые скиллы `mediaa-audit` и `mediaa-complexity` из `.agents/skills` и
`.claude/skills`.

## Новый скилл

`.agents/skills/mediaa-video-tests/SKILL.md` — единственный новый скилл; в
`.claude/skills/mediaa-video-tests/SKILL.md` лежит wrapper.

Скилл запускается **только по явному запросу пользователя**:

```powershell
cargo test -p mediaa-media --test pipeline scenarios -- --nocapture
```

Он не входит в обычную проверку кода; это зафиксировано в `specs/rules.md`.

## Метрики

Каждый `assert_mp4_snapshot` печатает:

```text
VIDEO_METRIC test=... snapshot=... difference=... quality=... pass=...
```

Качество теста — минимум `quality` по его источникам. Если тест упал, сравнительный документ
всё равно генерируется: ошибки накапливаются в `VideoCheck`, проверка срабатывает после
создания документа.

## Формат ответа агента

1. Таблица: тест, качество, успешно/упал.
2. Ссылки на сравнительные документы успешных тестов.
3. Ссылки на документы упавших тестов.

Без вступлений и лишних комментариев.

## Проверка

- media pipeline **51**;
- media lib **47**;
- app **268**;
- `clippy --workspace --all-targets` чистый.
