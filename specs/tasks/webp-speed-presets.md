# WebP: Direct-режим и пресеты скорости

## Контекст

Замеры 31.10.26 на 3200×2000/60 fps показали, что `Segmented` (один воркер) не быстрее
`Direct`, а иногда медленнее и всегда больше по размеру. `SegmentedParallel` даёт
нестабильный выигрыш по времени (в среднем ~0.98×) и раздувает файл в 2–4 раза из-за
keyframe на каждом сегменте. 15 из 50 прогонов ушли в fallback.

Решение: убрать сегментацию и live-нарезку. Остаётся один движок `Direct` и два пресета.

## Пресеты

- WebP `Quality` — `method 4`, fps и высота из настроек записи.
- WebP `Fast` — `method 0`, 30 fps, высота не больше 1080.

Пресет применяется и к захвату, и к `AnimSettings`, чтобы не писать лишние кадры.

### Единые пресеты записи

`RecordPreset` — вычисляемый пресет, `Custom` не хранится и не выбирается. В русском
UI он подписан «Свой», в английском — «Custom».

- Видео: Quality 60 fps / 2160p / 100%, Medium 60 / 1440 / 85, Fast 30 / 1080 / 70.
- Анимация: Quality 30 / 1080 / method 4 / Sierra2, Fast 30 / 720 / method 0 / Bayer.
- Аудио: Quality 256 kbps, Medium 192, Fast 128.
- Пресет — выпадающий список после формата, как разрешение; `Custom` показывается
  только когда текущие значения не совпадают ни с одним пресетом.
- По умолчанию: экран/видео — Quality, регион — Fast (30/720/method 0/Bayer),
  аудио — Quality (256 kbps).
- Смена пресета не меняет формат; у каждого формата свой набор настроек.
- `RecordSettings.saved_formats` хранит fps/height/quality/webp-mode/GIF-палитру по
  расширению формата. При переключении формата текущие значения сохраняются, а у
  нового формата восстанавливаются свои (или дефолтный пресет при первом выборе).
- Пресеты по форматам: видео — Quality/Medium/Fast, анимация — Quality/Fast.
- Любое ручное изменение полей, входящих в пресет, даёт `Custom`.
- Переключатель один и тот же на страницах записи App/Hotkey/Region и аудио.

## Что удалено

- `WebpMode::Segmented`, `WebpMode::SegmentedParallel`;
- `live_webp`, чанки, merge, `LivePaths`, `live_ok`;
- `WebpSegmentation`, `SegmentedWebpEncoder`, `FfmpegSegmentBackend`;
- fallback «segmented → linear»;
- тесты сегментов, merge и live-источников;
- настройки `segmented_webp`, `parallel_webp`.

## Что осталось

- `WebpMode::{Quality, Fast}` в `RecordSettings`;
- один полный проход FFmpeg/libwebp по MKV;
- `Effort::Fastest` → `compression_level=0`, `Effort::Balanced` → 4;
- нормализация still-WebP в `equalize_outputs`, чтобы несколько источников с разной
  длительностью не падали на `has no duration`;
- GIF-энкодер zengif как экспериментальный путь (`MEDIAA_GIF_ENCODER=zengif`).

## Замеры после переделки

36 прогонов, 6 сценариев × 3/5/10 с × Quality/Fast. Fast — 30 fps, 1080p, method 0.

| Сценарий | Quality | Fast | Ускорение |
|---|---|---|---|
| full 3s | 19,4 с | 4,6 с | 4,2× |
| full 5s | 31,1 с | 7,6 с | 4,1× |
| full 10s | 67,1 с | 15,1 с | 4,5× |
| two-windows 10s | 35,3 с | 13,2 с | 2,7× |
| one-window 10s | 22,7 с | 11,8 с | 1,9× |
| large 10s | 35,2 с | 12,6 с | 2,8× |

Среднее ускорение по всем 18 парам — 2,3×; полный экран — 4,1–4,5×. Размер у Fast
обычно меньше (full 10s: 4,16 → 2,18 МБ), кроме medium/small, где меняется содержимое.

## GIF

Текущий FFmpeg: full 3s — 10,9 с / 1,35 МБ; 10s — 38,7 с / 4,39 МБ.
zengif (quantizr, shared palette): full 3s native — 74 с, 30/1080 — 11,1 с против
5,3 с у FFmpeg. На одном окне и малом регионе — паритет, на полном экране zengif
в 2–7 раз медленнее. Вывод: zengif не внедряем, оставляем FFmpeg; экспериментальный
модуль и зависимость удалены.

## Проверки

- `cargo test -p mediaa-core -p mediaa-media`;
- `cargo test -p mediaa`;
- `cargo test -p mediaa-platform --lib`;
- `cargo fmt --check`.

## Покрытие тестами

- core: набор пресетов зависит от формата; точные значения видео/анимации/аудио;
  `Custom` при ручном изменении; смена пресета не меняет формат; MP4 720 → MKV 1080 →
  MP4 720; отдельные настройки WebP/GIF; сериализация `saved_formats`; дефолты
  экран/регион/аудио.
- UI: формат MP4/MKV восстанавливает свои значения; ручное качество даёт Custom;
  регион по умолчанию Fast; аудио по умолчанию Quality и становится Custom при смене
  битрейта.
