# Русская речь: четыре чтеца LibriVox, синтетическая беседа

* Лицензия: Общественное достояние: записи LibriVox переданы в общественное достояние — только такой материал
  кладём в репозиторий.
* Это **синтетическая беседа** с точным эталоном: куски четырёх чтецов одного
  обще-достояние-аудиокниги положены на одну шкалу времени. Каждый кусок принадлежит одному
  известному чтецу, поэтому `speakers.rttm` верен по построению, а не разметкой на слух.
* A synthetic conversation with an exact reference: pieces of four LibriVox readers cut from the same public-domain audiobook and laid on one timeline. Two readers are women, two are men; one reader has a 220 s monologue (the «clone» trap); four pieces start 1.2–1.8 s before the previous one ends, so those stretches are genuine overlaps.
* Сборка: `node scripts/make-diar-mix.mjs` (нужен `ffmpeg` в PATH или в `FFMPEG`; исходные
  mp3 кэшируются в `target/diar-src/`, в репозиторий попадает только `audio.opus`).
  Только план, без звука: `node scripts/make-diar-mix.mjs --plan`.
* Формат: моно, 16 кГц, Opus 32k, длительность 688.5 с.

## Источники

| Идентификатор archive.org | Название | Страница | Лицензия |
|---|---|---|---|
| `portraitsrussianpoets_1612_librivox` | Portraits of Russian Poets (Илья Эренбург, «Портреты русских поэтов») | https://archive.org/details/portraitsrussianpoets_1612_librivox | Общественное достояние: записи LibriVox переданы в общественное достояние (https://creativecommons.org/publicdomain/mark/1.0/) |

## Чтецы

| Ключ | Имя | Голос |
|---|---|---|
| `marinaf` | MarinaF | female |
| `maya-s` | Maya S | female |
| `mark-chulsky` | Mark Chulsky | male |
| `tovarisch` | tovarisch | male |

## Куски и шкала времени

| № | Кусок | Чтец | Файл источника | Смещение в источнике, с | Время в записи, с | Перекрытие |
|---|---|---|---|---|---|---|
| 1 | a1 | MarinaF | portraitsofrussianpoets_03_ehrenburg_128kb.mp3 | 30.0–42.0 | 0.0–12.0 | — |
| 2 | b1 | Maya S | portraitsofrussianpoets_07_ehrenburg_128kb.mp3 | 25.0–33.0 | 12.4–20.4 | — |
| 3 | a2 | MarinaF | portraitsofrussianpoets_03_ehrenburg_128kb.mp3 | 100.0–115.0 | 20.8–35.8 | — |
| 4 | d1 | tovarisch | portraitsofrussianpoets_13_ehrenburg_128kb.mp3 | 20.0–30.0 | 34.3–44.3 | 1.5 s |
| 5 | b2 | Maya S | portraitsofrussianpoets_07_ehrenburg_128kb.mp3 | 70.0–90.0 | 44.7–64.7 | — |
| 6 | c1 | Mark Chulsky | portraitsofrussianpoets_05_ehrenburg_128kb.mp3 | 20.0–45.0 | 65.1–90.1 | — |
| 7 | a3 | MarinaF | portraitsofrussianpoets_03_ehrenburg_128kb.mp3 | 200.0–207.0 | 90.5–97.5 | — |
| 8 | b3 | Maya S | portraitsofrussianpoets_07_ehrenburg_128kb.mp3 | 120.0–150.0 | 96.3–126.3 | 1.2 s |
| 9 | d2 | tovarisch | portraitsofrussianpoets_13_ehrenburg_128kb.mp3 | 60.0–78.0 | 126.7–144.7 | — |
| 10 | c2 | Mark Chulsky | portraitsofrussianpoets_05_ehrenburg_128kb.mp3 | 60.0–280.0 | 145.1–365.1 | — |
| 11 | a4 | MarinaF | portraitsofrussianpoets_03_ehrenburg_128kb.mp3 | 300.0–310.0 | 365.5–375.5 | — |
| 12 | d3 | tovarisch | portraitsofrussianpoets_13_ehrenburg_128kb.mp3 | 110.0–155.0 | 375.9–420.9 | — |
| 13 | b4 | Maya S | portraitsofrussianpoets_07_ehrenburg_128kb.mp3 | 200.0–212.0 | 421.3–433.3 | — |
| 14 | a5 | MarinaF | portraitsofrussianpoets_03_ehrenburg_128kb.mp3 | 420.0–445.0 | 431.5–456.5 | 1.8 s |
| 15 | c3 | Mark Chulsky | portraitsofrussianpoets_05_ehrenburg_128kb.mp3 | 300.0–314.0 | 456.9–470.9 | — |
| 16 | d4 | tovarisch | portraitsofrussianpoets_13_ehrenburg_128kb.mp3 | 180.0–188.0 | 471.3–479.3 | — |
| 17 | b5 | Maya S | portraitsofrussianpoets_07_ehrenburg_128kb.mp3 | 270.0–310.0 | 479.7–519.7 | — |
| 18 | a6 | MarinaF | portraitsofrussianpoets_03_ehrenburg_128kb.mp3 | 520.0–538.0 | 520.1–538.1 | — |
| 19 | c4 | Mark Chulsky | portraitsofrussianpoets_05_ehrenburg_128kb.mp3 | 10.0–65.0 | 538.5–593.5 | — |
| 20 | d5 | tovarisch | portraitsofrussianpoets_13_ehrenburg_128kb.mp3 | 230.0–265.0 | 592.3–627.3 | 1.2 s |
| 21 | b6 | Maya S | portraitsofrussianpoets_07_ehrenburg_128kb.mp3 | 370.0–380.0 | 627.7–637.7 | — |
| 22 | a7 | MarinaF | portraitsofrussianpoets_03_ehrenburg_128kb.mp3 | 590.0–620.0 | 638.1–668.1 | — |
| 23 | c5 | Mark Chulsky | portraitsofrussianpoets_05_ehrenburg_128kb.mp3 | 315.0–335.0 | 668.5–688.5 | — |
