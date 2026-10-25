use super::*;

pub(super) fn start(app: &mut App, ctx: &egui::Context, target: Target) {
    let Some(input) = app.conv.input.clone() else {
        return;
    };
    let folder = app
        .conv
        .folder
        .clone()
        .unwrap_or_else(|| app.settings.convert_dir());
    let output = convert::output_path(
        &mediaa_core::paths::SourcePath::new(input.clone()),
        target,
        Some(&app.conv.name),
        &mediaa_core::paths::MediaDir::new(folder),
    );
    let settings = app.conv.effective();
    app.conv.estimate = None;
    let progress = Arc::new(AtomicI32::new(-1));
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = crossbeam_channel::bounded(1);
    let (p, c, out, ctx2) = (
        progress.clone(),
        cancel.clone(),
        output.clone(),
        ctx.clone(),
    );
    let thread = app.tasks.spawn(
        TaskKind::Conversion,
        "convert",
        Some(cancel.clone()),
        move || {
            let mut report = |f: Option<f32>| {
                p.store(f.map_or(-1, |f| (f * 1000.0) as i32), Ordering::Relaxed);
                ctx2.request_repaint();
            };
            let source = mediaa_core::paths::SourcePath::new(input);
            let r = match convert::convert(&source, &out, target, &settings, &mut report, &c) {
                Ok(()) => ConversionTaskResult::Completed,
                Err(error) if mediaa_media::job::is_cancelled(&error) => {
                    ConversionTaskResult::Cancelled
                }
                Err(error) => ConversionTaskResult::Failed {
                    kind: classify_media_error(&error),
                    message: format!("{error:#}"),
                },
            };
            let _ = tx.send(r);
            ctx2.request_repaint();
        },
    );
    match thread {
        Ok(task) => {
            app.conv.job = Some(Job {
                output,
                progress,
                started: Instant::now(),
                cancel,
                done: rx,
                task,
            });
        }
        Err(error) => app.error(t!("convert.start_failed", error = error)),
    }
}

pub(super) fn update_estimate(app: &mut App, ctx: &egui::Context, target: Target) {
    let Some(input) = app.conv.input.clone() else {
        return;
    };
    let key = (input, target, app.conv.effective());
    let conv = &mut app.conv;
    if conv.estimate.as_ref().is_none_or(|e| e.key != key) {
        let previous = conv.estimate.as_ref().and_then(|e| {
            e.result
                .as_ref()
                .and_then(|r| r.as_ref().ok().copied())
                .or(e.previous)
        });
        conv.estimate = Some(Estimate {
            key,
            since: Instant::now(),
            cancel: Arc::default(),
            rx: None,
            result: None,
            previous,
            task: None,
        });
    }
    let busy = conv.job.is_some();
    let Some(e) = conv.estimate.as_mut() else {
        return;
    };
    if e.result.is_some() {
        return;
    }
    match &e.rx {
        Some(rx) => match rx.try_recv() {
            Ok(result) => {
                e.result = Some(result);
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                e.result = Some(Err(t!("convert.worker_stopped").into()));
            }
        },
        None if busy => {}
        None if e.since.elapsed() < ESTIMATE_DELAY => ctx.request_repaint_after(ESTIMATE_DELAY),
        None => {
            let (tx, rx) = crossbeam_channel::bounded(1);
            let (key, cancel, ctx) = (e.key.clone(), e.cancel.clone(), ctx.clone());
            let spawned = app.tasks.spawn_with_deadline(
                TaskKind::ConversionEstimate,
                "convert-estimate",
                Some(cancel.clone()),
                Some(super::ESTIMATE_TIMEOUT),
                move || {
                    let (input, target, settings) = key;
                    let input = mediaa_core::paths::SourcePath::new(input);
                    let r = convert::estimate_size(&input, target, &settings, &cancel);
                    if !cancel.load(Ordering::Relaxed) {
                        let _ = tx.send(r.map_err(|e| format!("{e:#}")));
                        ctx.request_repaint();
                    }
                },
            );
            match spawned {
                Ok(task) => {
                    e.rx = Some(rx);
                    e.task = Some(task);
                }
                Err(error) => e.result = Some(Err(format!("estimate: {error}"))),
            }
        }
    }
    if e.result.is_none() && e.task.is_some_and(|task| app.tasks.overdue(task)) {
        e.cancel.store(true, Ordering::Relaxed);
        e.result = Some(Err(t!("convert.estimate_timeout").into()));
        e.rx = None;
        if let Some(task) = e.task.take() {
            app.tasks.detach(task);
        }
    }
    if e.result.is_some() {
        if let Some(task) = e.task.take() {
            app.tasks.finish(task);
        }
    }
}
