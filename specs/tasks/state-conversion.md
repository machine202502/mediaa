# ST-4: Conversion — один controller вместо двух и флагов

## Где

- `crates/backend/src/features/conversion/mod.rs`
- `crates/frontend/src/features/conversion/mod.rs`

Почти одинаковые `ConversionController` живут в двух крейтах. Оба хранят
`job`, `batch_auto`, `probe_loading`, `conflict_notified`, `failed`,
`run_started`, `probed`, `probe_job`.

## Текущее состояние

```rust
pub struct ConversionController {
    pub input: Option<PathBuf>,
    pub queue: Vec<PathBuf>,
    pub job: Option<ConversionJob>,
    pub batch_auto: bool,
    pub failed: Vec<(PathBuf, String)>,
    pub batch: Vec<BatchItem>,
    pub conflict_notified: bool,
    pub probed: Option<ProbeResult>,
    pub probe_job: Option<Job<ProbeResult>>,
    pub probe_loading: bool,
    pub target: Option<Target>,
    pub run_started: Option<Instant>,
}
```

Один и тот же факт «идёт конвертация» кодируется через `job.is_some()`,
`batch_auto` и `run_started`. `probe_loading` дублирует `probe_job.is_some()`.
`conflict_notified` — это не состояние, а факт «уже показали ошибку».

`BatchStatus` — хороший enum, но рядом с ним живут `failed` и
`conflict_notified`.

## Целевая модель

```rust
enum ConversionState {
    Empty,
    Probing { request: u64 },
    Ready { source: PathBuf, target: Target },
    Running { job: ConversionJob, batch: Vec<BatchItem> },
    BatchFinished { batch: Vec<BatchItem> },
    Failed { message: String },
}
```

События:

```rust
enum ConversionEvent {
    RequestProbe(PathBuf),
    ProbeFinished(ProbeResult),
    ProbeFailed(String),
    Start,
    Progress(i32),
    ItemFinished { path: PathBuf, outcome: ConversionOutcome },
    Cancel,
    Reset,
}
```

Эффекты:

```rust
enum ConversionEffect {
    SpawnProbe(PathBuf),
    SpawnJob(ConversionRequest),
    CancelJob,
    Notify(ConversionNotice),
    Publish,
}
```

`BatchItem.status` остаётся, но это состояние элемента, а не контроллера.
`failed` вычисляется из `batch`, `conflict_notified` заменяется событием
`NotifyOnce`/проверкой в `ConversionEffect`.

## Тесты (без Windows)

1. `probe_moves_empty_to_probing`
2. `probe_result_moves_to_ready`
3. `start_moves_ready_to_running`
4. `progress_does_not_change_phase`
5. `item_done_keeps_running_until_all_items`
6. `last_item_moves_to_batch_finished`
7. `cancel_moves_running_to_empty`
8. `start_while_running_is_rejected`
9. `failed_items_are_derived_from_batch`
10. `reset_clears_finished_batch`

## Критерии готовности

- Нет `batch_auto`, `probe_loading`, `conflict_notified`.
- `job.is_some()` не используется как фаза.
- `failed` не хранится отдельно.
- Backend и frontend используют одно и то же имя состояний.
- `cargo test -p mediaa-backend conversion` и
  `cargo test -p mediaa-frontend conversion` зелёные.
