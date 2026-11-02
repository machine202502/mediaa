# ST-3: Capture / overlay — состояние захвата и редактора

## Где

- `crates/frontend/src/features/capture.rs`
- `crates/frontend/src/app/capture.rs`
- `crates/frontend/src/ui/region_editor.rs`
- `crates/frontend/src/features/capture_client.rs`

## Текущее состояние

`CaptureController` держит три независимых `Option<Job>`:

- `capture: Option<CaptureJob<K>>`
- `screenshot: Option<ScreenshotJob>`
- `copy_image: Option<CopyImageJob>`

`busy()` — это `capture.is_some() || screenshot.is_some() || copy_image.is_some()`.
То есть фаза вычисляется из трёх полей.

Дальше флаги-намерения:

- `after_completion: bool`
- `copies_file: bool`
- `restore: bool`
- `hide: bool`
- `copy_file: bool`
- `copy_image: bool`
- `CapturedImage.action`, `format`, `after_completion`
- `App.overlay: Option<Overlay>` и `region_outline`.

Состояния «захват идёт», «скриншот сохраняется», «картинка копируется»,
«редактор открыт», «редактор закрывается» смешаны.

## Целевая модель

```rust
enum CaptureState {
    Idle,
    Capturing { kind: CaptureKind, started: Instant },
    Editing { image: CapturedImage, selection: Rect },
    Saving { image: CapturedImage, copy: CopyMode },
    CopyingImage,
    Done { notice: CaptureNotice },
    Failed { message: String },
}
```

События:

```rust
enum CaptureEvent {
    StartCapture(CaptureKind),
    FrameCaptured(Captured),
    CaptureTimedOut,
    CaptureFailed(String),
    EditorConfirmed,
    EditorCancelled,
    SaveFinished(ScreenshotDone),
    CopyFinished(Result<(), String>),
    Tick(Instant),
}
```

Эффекты:

```rust
enum CaptureEffect {
    SpawnCapture(CaptureKind),
    OpenEditor(CapturedImage),
    SpawnSave(ScreenshotWork),
    SpawnCopyImage,
    CloseEditor,
    Notify(CaptureNotice),
    RestoreWindow,
    HideWindow,
}
```

`after_completion`/`restore`/`hide` — это параметры эффекта, а не поля
контроллера. `overlay` — часть `Editing`, а не отдельный `Option` в `App`.

## Тесты (без Windows)

1. `start_capture_moves_idle_to_capturing`
2. `second_capture_while_capturing_is_rejected`
3. `frame_moves_capturing_to_editing`
4. `timeout_moves_capturing_to_failed`
5. `editor_cancel_returns_to_idle`
6. `confirm_moves_editing_to_saving`
7. `save_done_moves_to_done_and_closes_editor`
8. `copy_image_and_screenshot_do_not_run_together`
9. `restore_is_an_effect_not_state`

## Критерии готовности

- В контроллере нет трёх независимых `Option<Job>`.
- `busy()` — метод над enum.
- `App.overlay` заменён на `CaptureState::Editing`.
- Все переходы покрыты тестами.
- `cargo test -p mediaa-frontend capture` зелёный.
