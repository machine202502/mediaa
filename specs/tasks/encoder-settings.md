# Настройки потоков и энкодеров: отложенная задача

Статус: отложено (02.11.26). Документ для следующего исполнителя: что уже есть в коде,
что можно настраивать, на что это влияет и как аккуратно добавить это в UI.

## 0. Решение, которое принято сейчас

- Пользовательских настроек потоков, lookahead, пресетов и т.п. **нет**.
- В коде уже есть `EncoderTuning` в `RecordSettings`; это задел, а не UI.
- Для видео по умолчанию включён `zerolatency`; это выбрано ради памяти, а не ради
  минимального размера файла.
- GIF переведён на two-pass палитру (с автоматическим one-pass для маленьких файлов);
  потоки при этом не ограничиваются.
- Потоки **не трогаем**, пока пользователь не попросит: качество и число кадров важнее
  скорости сохранения.

Этот документ описывает будущую работу, чтобы её можно было делать без повторного
исследования кода и замеров.

## 1. Где что кодируется

### 1.1. MP4/MKV

Запись идёт в два этапа:

1. **Во время записи** capture loop пишет scratch-файл `video-*.mkv`:
   - `crates/media/src/recorder/capture.rs`, `scratch_output`;
   - `OutputKind::Video` → `libx264`, preset из `filters::live_preset`,
     CRF из `filters::crf_for_quality`;
   - сюда применяется `EncoderTuning`.
2. **Финализация**:
   - `crates/media/src/convert.rs`, `mux_recording`;
   - видео копируется (`copy_stream_map("0:v")`), без повторного x264;
   - при наличии звука кодируется AAC (`audio_kbps`);
   - поэтому потоки на финализации MP4/MKV почти ни на что не влияют:
     размер файла уже определён на этапе записи.

Важно: для MP4/MKV настройки «потоки/lookahead/preset/CRF» — это **настройки
захвата**, а не финализации. `-tune zerolatency` влияет на размер файла и память
именно на этой стадии.

### 1.2. GIF

1. Во время записи — тот же scratch, но для `OutputKind::Anim`:
   - `libx264rgb`, `preset ultrafast`, `qp=0` (lossless);
   - `scratch_graph("bgr0", max_height)`;
   - `EncoderTuning` применяется к этому scratch-энкодеру.
2. Финализация — `crates/media/src/animation_backends.rs` →
   `mediaa_gif::encode`:
   - two-pass: `palettegen` → временный PNG, затем `paletteuse` потоком;
   - one-pass для маленьких файлов (порог 128 MiB оценки RGBA×кадры);
   - `mediaa_gif::encode` умеет принимать `threads`, но рекордер сейчас передаёт
     `None` (см. `AnimationEncodeRequest { threads: None }`);
   - `threads` в `mediaa_gif::encode` применяется к **входному декодеру**, не к
     самому GIF-энкодеру;
   - `palettegen`/`paletteuse` — фильтры FFmpeg; отдельного пользовательского
     параметра потоков для них сейчас нет.

### 1.3. WebP

1. Во время записи — тот же lossless scratch, `EncoderTuning`.
2. Финализация — `crates/media/src/animation_backends.rs` →
   `mediaa_webp::encode`:
   - `EncodeOptions { quality, compression_level, threads }`;
   - рекордер передаёт `threads: None`;
   - `WebpMode::Fast` → compression level 0, `Quality` → 4;
   - `threads` применяется к входному декодеру и к `libwebp_anim`.

### 1.4. Звук

Аудио-дорожки пишутся отдельно (`audio-*.wav`) и на финализации кодируются AAC/…;
потоки видео-энкодера к ним не относятся. Настройки аудио — отдельная тема.

## 2. Текущая модель настроек

`crates/core/src/settings.rs`:

```rust
pub struct EncoderTuning {
    pub zerolatency: bool,   // default true
    pub threads: Option<u32>, // None = auto
    pub lookahead: Option<u32>,
}

pub struct RecordSettings {
    // output, fps, quality, output_height, ...
    pub encoder_tuning: EncoderTuning,
}
```

- `RecordSettings` есть в трех профилях: `record`, `hotkey_record`,
  `region_record`; у каждого свой `encoder_tuning`.
- `crates/app/src/logic/recording/plan.rs` передаёт
  `record.encoder_tuning` в `RecordingRequest`.
- `crates/media/src/recorder/capture.rs` применяет:
  - `tune=zerolatency` при `zerolatency=true`;
  - `threads=N` при `Some(N)`;
  - `rc-lookahead=N` только если `zerolatency=false` (при zerolatency
    lookahead игнорируется).
- Значения сериализуются в настройках, синхронизируются через `settings` store,
  но **в UI не показаны**.

`EncoderTuning` относится к **capture**, а не к финализации GIF/WebP.
`AnimationEncodeRequest.threads` — отдельное поле, сейчас всегда `None`.

## 3. Что на что влияет

### 3.1. x264 threads (MP4/MKV и lossless scratch)

- Больше потоков: быстрее захват, больше памяти (буферы кадров).
- Меньше потоков: медленнее, меньше памяти.
- На качество при том же CRF почти не влияет; на размер — слабо (frame threading
  может немного менять решения rate control, но это не «качество»).
- Опасность при **захвате**: если потоков слишком мало и энкодер не успевает,
  capture loop может пропускать слоты — это не падение качества, а пропуски
  кадров/времени. `SlotTracker` в `crates/core/src/pacer.rs` допускает разрывы:
  при отставании следующий слот прыгает вперёд.
- `threads=0`/`None` — auto (FFmpeg сам считает по ядрам). В коде сейчас `None`.

### 3.2. lookahead / B-кадры / zerolatency

- Lookahead и B-кадры улучшают сжатие: файл меньше при том же визуальном
  качестве.
- `zerolatency` выключает lookahead/B-кадры (и включает sliced threads):
  память сильно падает, файл растёт.
- При `zerolatency=true` `lookahead` не имеет смысла; UI должен либо скрывать,
  либо явно показывать «игнорируется».
- `zerolatency` влияет на размер MP4/MKV и на память **захвата**; на GIF/WebP
  финализацию — только через lossless scratch.

### 3.3. Пресет x264

- Сейчас выбирается автоматически: `filters::live_preset`:
  - очень большой поток (≈>1440p60) — `superfast`;
  - иначе — `veryfast`.
- Пресет — компромисс скорость/сжатие. В будущем можно дать пользователю
  выбор, но это отдельная настройка, не threads.
- Не путать с `Effort` для WebP и с `quality` (CRF).

### 3.4. CRF / video_quality

- `video_quality` 1..100 → `filters::crf_for_quality`.
- Сильнее всего влияет на размер MP4/MKV.
- Не является настройкой потоков; в UI уже есть.

### 3.5. GIF threads

- Сейчас auto; ограничение потоков **декодера** уменьшит память, но замедлит
  финализацию.
- На количество кадров и палитру не влияет: кадры уже записаны в scratch.
- Остаточная память 4K GIF (~1 GB) в основном от автопотоков декодера.
- `palettegen`/`paletteuse` могут использовать threading FFmpeg; отдельного
  per-filter параметра нет. Нужно проверить, что `-threads` на входе достаточно.

### 3.6. WebP threads

- Влияет на декодер и `libwebp_anim`; память/скорость.
- На качество при том же `quality`/`compression_level` не влияет.
- `WebpMode` (Fast/Quality) сильнее влияет на время/размер, чем threads.

## 4. Замеры (02.11.26, debug, Windows 11, 20 логических ядер, 32 GB)

Синтетический подвижный паттерн, MP4 30 fps, если не указано иное. Цифры — пик RSS
процесса, полное время включает запись.

| Формат | 854×480 | 1920×1080 | 3840×2160 |
|---|---:|---:|---:|
| MP4, zerolatency | 39.7 MB / 4.13 s | 134.5 MB / 4.17 s | 435.3 MB / 3.28 s |
| WebP fast | 36.3 MB / 4.36 s | 511.1 MB / 5.92 s | 1282.5 MB / 10.70 s |
| GIF two-pass, 30 fps | 78.3 MB / 4.47 s | 491.1 MB / 5.53 s | 1074.1 MB / 5.02 s (2 с) |
| GIF one-pass (было) | 250.8 MB / 4.46 s | 1012.1 MB / 5.06 s | 2140.4 MB / 4.20 s (2 с) |

Для 4K MP4/WebP в таблице — 3 секунды записи, для 4K GIF — 2 секунды.

GIF 1080p 15 fps: 384 MB two-pass (было 682 MB). GIF 480p 15 fps: 37 MB one-pass
(оставлен one-pass, two-pass был бы 47 MB).

MP4 1080p: `zerolatency` 395 KB против 118 KB с авто-потоками и lookahead
(пик 134 MB против 778 MB). Это цена памяти за размер файла.

FFmpeg CLI, 1080p, x264 veryfast, только для сравнения энкодера:
авто 742 MB; `-threads 4` 232 MB; `-threads 8` 287 MB; `-tune zerolatency` 62 MB.
4K: авто ≈2.6 GB; `-threads 4` 811 MB; `zerolatency` 166 MB.

Ограничение: синтетика, не реальный рабочий стол; абсолютные цифры могут отличаться,
но зависимости от разрешения/потоков/zerolatency устойчивы.

## 5. Что именно нужно добавить в будущем

### 5.1. Модель настроек

Не добавлять всё в `EncoderTuning` без разделения. Предлагаемая форма:

```rust
pub struct CaptureEncoderSettings {
    pub threads: ThreadSetting,      // Auto | Fixed(u32)
    pub zerolatency: bool,
    pub lookahead: Option<u32>,
    // preset — отдельно, если понадобится
}

pub struct AnimationEncoderSettings {
    pub threads: ThreadSetting,      // decoder/finalizer
}

pub struct EncoderSettings {
    pub capture: CaptureEncoderSettings,
    pub gif: AnimationEncoderSettings,
    pub webp: AnimationEncoderSettings,
}
```

Варианты размещения:

- глобально в `Settings` — проще для пользователя;
- per-profile в `RecordSettings` — регион/хоткей/экран могут иметь разные
  значения; это уже текущая структура.

Рекомендация: глобальные значения + необязательные override на профиль, чтобы не
плодить три копии в UI. Решение за исполнителем, но зафиксировать в задаче.

### 5.2. Куда прокинуть

1. `RecordingRequest`:
   - capture threads/lookahead/zerolatency уже через `encoder_tuning`;
   - если добавляется preset — отдельное поле, не ломать `EncoderTuning`.
2. `capture_loop` / `scratch_output`:
   - применить `threads`, `tune`, `lookahead`;
   - сохранить текущую логику «lookahead только без zerolatency».
3. `recorder.rs` при создании `AnimationEncodeRequest`:
   - передавать `threads` из настроек GIF/WebP, а не `None`;
   - `mediaa_gif::encode` применит его к декодеру в обоих проходах;
   - `mediaa_webp::encode` применит к декодеру и `libwebp_anim`.
4. `mediaa_gif`:
   - убедиться, что threads применяется и в palette-pass, и в encode-pass;
   - проверить, что one-pass и two-pass дают одинаковые кадры/палитру.
5. MP4/MKV finalize:
   - threads видео не нужны (stream copy); не добавлять фиктивные настройки.
   - Если появится перекодирование (trim/convert) — это другая задача.

### 5.3. UI

Пока UI нет; при добавлении:

- Advanced settings, отдельная секция «Производительность кодирования»;
- для каждого формата: Auto / Fixed threads;
- предупреждение: слишком мало потоков на 4K/60 fps может привести к пропуску
  кадров при захвате;
- показывать, что `lookahead` не действует при `zerolatency`;
- не обещать «качество» от threads — только скорость/память; качество
  определяется CRF/палитрой/dither/разрешением;
- сброс к defaults; значения по умолчанию — текущие (Auto + zerolatency для
  capture, Auto для GIF/WebP).

### 5.4. Совместимость

- `Settings` сериализуется с `#[serde(default)]`; новые поля должны иметь
  безопасные defaults, чтобы старые `.mediaa.toml` читались.
- Settings синхронизируются через protocol store; после добавления полей
  проверить roundtrip и то, что старый клиент/новый демон не падает.
- `EncoderTuning` уже сериализуется; не переименовывать поля без миграции.

## 6. Тестовая матрица для будущей реализации

1. `Settings` roundtrip: старый файл без новых полей → defaults; новый файл → все
   значения.
2. MP4 1080p:
   - threads Auto / 4 / 1;
   - проверить кадры (`frames_captured` ≈ fps×duration), размер, RSS;
   - `zerolatency=false` + lookahead=10.
3. MP4 4K: threads Auto / 4; проверить, что нет пропусков кадров при 30 fps.
4. GIF 1080p:
   - threads Auto / 2;
   - two-pass и one-pass;
   - сравнить число кадров, размер, хеш/палитру с текущим;
   - убедиться, что качество не зависит от threads.
5. GIF 4K 2 с: threads Auto / 2; зафиксировать RSS/время/размер.
6. WebP 1080p/4K: threads Auto / 2; сравнить размер/время/кадры.
7. Region recording:
   - маленький регион (например 640×480) → one-pass/low memory;
   - большой регион 2560×1440 → two-pass;
   - настройки per-profile применяются к нужному профилю.
8. Обрыв/отмена во время финализации: временный palette PNG удаляется, исходники
   не теряются.
9. UI-тест: значения сохраняются, применяются, сброс работает.

## 7. Открытые вопросы

- Нужны ли отдельные настройки для GIF и WebP, или одна «финализация анимаций»?
- Нужен ли `threads=0` как явный Auto, или `None` достаточно?
- Поддерживает ли `palettegen`/`paletteuse` per-filter threads; нужно проверить
  на реальном 4K, а не только `-threads` входа.
- Влияет ли ограничение потоков декодера на точный хеш GIF/WebP? Ожидается «нет»,
  но проверить на одном и том же scratch.
- Нужен ли пользователю preset x264, или достаточно CRF + threads + lookahead?
- Как показывать оценку памяти в UI (формула RGBA×frames + потоки) — приблизительно
  или по реальным замерам?
- Нужны ли аппаратные энкодеры (NVENC/QSV/AMF) — отдельная задача, не смешивать.

## 8. Файлы-ориентиры

- `crates/core/src/settings.rs` — `EncoderTuning`, `RecordSettings`.
- `crates/app/src/logic/recording/plan.rs` — выбор профиля и запроса.
- `crates/media/src/recorder/request.rs` — builder.
- `crates/media/src/recorder/capture.rs` — scratch-энкодер.
- `crates/media/src/recorder.rs` — finalize, `AnimationEncodeRequest`.
- `crates/media/src/animation_backends.rs` — GIF/WebP backend.
- `crates/gif/src/encode.rs`, `crates/gif/src/graph.rs` — two-pass палитра.
- `crates/webp/src/encode.rs` — `EncodeOptions`.
- `crates/media/src/filters.rs` — preset/CRF/Effort.
- `crates/core/src/pacer.rs` — пропуски слотов при отставании захвата.

## 9. Что не делать без отдельного решения

- Не включать пользовательские потоки «по умолчанию» без UI и предупреждений.
- Не менять `zerolatency` дефолт без замера размера файла.
- Не ограничивать потоки на захвате 4K/60 без проверки пропусков кадров.
- Не обещать, что threads улучшат качество: они влияют на скорость/память.
- Не путать GIF two-pass (палитра) с threads: это разные оптимизации.
