# ST-5: Library / player — убрать флаги воспроизведения

## Где

- `crates/frontend/src/features/library/player.rs`
- `crates/frontend/src/features/library/viewer.rs`
- `crates/frontend/src/features/library/mod.rs`
- `crates/frontend/src/features/library/view.rs`

## Текущее состояние

`Player` — набор независимых флагов и счётчиков:

- `paused: bool`
- `got_frame: bool`
- `pending: Option<PreviewFrame>`
- `audio_pending: bool`
- `audio_waiting: bool`
- `audio_needs_alignment: bool`
- `dragging: bool`
- `video_generation: u64`
- `audio_generation: u64`
- `anchor: Option<Instant>`

Из них складываются состояния «пауза», «ждём первый кадр», «ждём аудио»,
«перетаскиваем», «переключили файл». Комбинации не проверяются: можно
одновременно `audio_waiting` и `paused`, `got_frame=false` и `pending=Some`.

`Viewer` держит `image_job` и `audio_job` как два независимых `Option`.

## Целевая модель

```rust
enum PlayerState {
    Idle,
    Loading { started: Instant, want_audio: bool },
    Ready { paused: bool, pos: f64, anchor: Option<Instant> },
    Dragging { from: f64, started: Instant },
    Closed,
}
```

События:

```rust
enum PlayerEvent {
    Open(PathBuf),
    FirstFrame(PreviewFrame),
    AudioReady,
    AudioFailed(String),
    TogglePause,
    Seek(f64),
    DragStart,
    DragEnd,
    Tick(Instant),
    Close,
}
```

Эффекты:

```rust
enum PlayerEffect {
    StartVideo(PathBuf),
    StartAudio(PathBuf),
    SeekVideo(f64),
    SeekAudio(f64),
    StopAudio,
    ShowFrame(PreviewFrame),
    Publish,
}
```

Генерации остаются как защита от stale-кадров, но не как состояние.
`got_frame`/`audio_waiting`/`audio_needs_alignment` заменяются фазой
`Loading` и её параметрами.

## Тесты (без Windows и без реального видео)

1. `open_moves_idle_to_loading`
2. `first_frame_moves_loading_to_ready`
3. `audio_failure_still_allows_video`
4. `pause_only_in_ready`
5. `drag_start_and_end_restore_ready`
6. `stale_frame_from_old_generation_is_ignored`
7. `close_stops_workers_and_moves_to_closed`
8. `seek_during_loading_is_queued_or_rejected`
9. `image_viewer_and_audio_job_do_not_race`

## Критерии готовности

- `Player` не имеет независимых `got_frame/audio_*` флагов.
- `Viewer` не держит два `Option<Job>` как фазу.
- Все переходы покрыты тестами.
- `cargo test -p mediaa-frontend player` зелёный.
