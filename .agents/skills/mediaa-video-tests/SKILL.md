---
name: mediaa-video-tests
description: "Запускает 30-секундные MP4-видео-тесты MediaA и формирует короткий структурированный отчёт: таблица тестов с качеством и статусом, ссылки на сравнительные документы успешных и упавших тестов. Использовать только по явному запросу пользователя; не запускать в обычной проверке кода."
---

# MediaA: видео-тесты

## Когда запускать

Только по явному запросу пользователя. Не запускать в рамках обычной проверки изменений,
`cargo test` для Rust-кода или других стандартных прогонов.

## Запуск

```powershell
cargo test -p mediaa-media --test pipeline scenarios -- --nocapture
```

Метрики в выводе имеют вид:

```text
VIDEO_METRIC test=three_windows snapshot=thirty-window-1.mp4 difference=... quality=... pass=true
```

Один тест может дать несколько `VIDEO_METRIC` — по одному на выходной источник. Качество теста
бери как **минимум** `quality` среди его строк. `pass` — все строки `pass=true`.

## Сравнительные документы

После каждого теста генерируются MP4-документы в:

```text
target/comparative-docs/
```

Они вне Git. Сопоставление:

- `three_monitors` → `target/comparative-docs/three-monitors.mp4`
- `three_monitors_disconnect` → `target/comparative-docs/three-monitors-disconnect.mp4`
- `three_windows` → `target/comparative-docs/three-windows.mp4`
- `single_window_fake_on` → `target/comparative-docs/single-window-fake-on.mp4`
- `single_window_fake_off` → `target/comparative-docs/single-window-fake-off.mp4`
- `closed_two_windows` → `target/comparative-docs/closed-two-windows.mp4`
- `two_windows_close_fake_off` → `target/comparative-docs/two-windows-close-fake-off.mp4`

## Ответ пользователю

Без вступлений, выводов и лишних комментариев. Ровно три раздела.

### 1. Результаты

| Тест | Качество | Статус |
|---|---:|---|
| ... | 99.2% | ✅ успешно |
| ... | 42.0% | ❌ упал |

### 2. Успешные тесты

Ссылки на сравнительные документы только успешных тестов:

- [Название](target/comparative-docs/файл.mp4)

### 3. Упавшие тесты

Ссылки на сравнительные документы только упавших тестов:

- [Название](target/comparative-docs/файл.mp4)

Если упавший тест не успел создать документ, укажи `документ не создан`.
