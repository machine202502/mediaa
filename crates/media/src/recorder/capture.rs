use super::*;

fn scratch_output(
    path: &Path,
    live: Option<&LivePaths>,
    kind: OutputKind,
    quality: u8,
    max_height: Option<u32>,
    size: (u32, u32),
    fps: u32,
) -> (String, Output) {
    let out = match live {
        Some(live) => Output::from(live.template.to_string_lossy().into_owned())
            .set_format("segment")
            .set_format_opt("segment_format", "matroska")
            .set_format_opt("segment_time", "1")
            .set_format_opt("segment_list", live.list.to_string_lossy().into_owned())
            .set_format_opt("segment_list_type", "flat")
            .set_format_opt("reset_timestamps", "1")
            .add_stream_map("[v]"),
        None => Output::from(path.to_string_lossy().into_owned())
            .set_format("matroska")
            .add_stream_map("[v]"),
    };
    match kind {
        OutputKind::Video(_) => (
            filters::scratch_graph("yuv420p", max_height),
            out.set_video_codec("libx264")
                .set_video_codec_opt("preset", {
                    let (w, h) = filters::scaled_size(size.0, size.1, max_height);
                    filters::live_preset(w, h, fps)
                })
                .set_video_codec_opt("crf", filters::crf_for_quality(quality).to_string()),
        ),
        OutputKind::Audio(_) => unreachable!("audio recordings have no picture to encode"),
        OutputKind::Anim(_) => {
            let out = out
                .set_video_codec("libx264rgb")
                .set_video_codec_opt("preset", "ultrafast")
                .set_video_codec_opt("qp", "0");
            let out = if live.is_some() {
                out.set_video_codec_opt("g", fps.to_string())
                    .set_video_codec_opt("keyint_min", fps.to_string())
                    .set_video_codec_opt("sc_threshold", "0")
            } else {
                out
            };
            (filters::scratch_graph("bgr0", max_height), out)
        }
    }
}

#[allow(clippy::too_many_arguments, reason = "plain data handed to a thread")]
pub(super) fn capture_loop(
    factory: SourceFactory,
    control: Arc<Control>,
    fps: u32,
    kind: OutputKind,
    quality: u8,
    max_height: Option<u32>,
    live: Option<LivePaths>,
    scratch: PathBuf,
    frames: Arc<AtomicU64>,
    ready: Sender<anyhow::Result<(String, (u32, u32))>>,
) -> anyhow::Result<Vec<PathBuf>> {
    let setup = (|| -> anyhow::Result<_> {
        if let Some(live) = &live {
            std::fs::create_dir_all(&live.dir)?;
        }
        let source = factory()?;
        let (w, h) = source.size();
        let (push_source, handle) = FramePushSource::builder(w, h)
            .pixel_format("rgba")
            .time_base(1, fps as i32)
            .build()?;
        let (graph, output) = scratch_output(
            &scratch,
            live.as_ref(),
            kind,
            quality,
            max_height,
            (w, h),
            fps,
        );
        let scheduler = FfmpegContext::builder()
            .input(push_source)
            .filter_desc(graph)
            .output(output)
            .build()?
            .start()?;
        Ok((source, handle, scheduler))
    })();
    let (mut source, mut handle, scheduler) = match setup {
        Ok(v) => {
            let size = v.0.size();
            let _ = ready.send(Ok((v.0.name(), size)));
            v
        }
        Err(e) => {
            let _ = ready.send(Err(anyhow::anyhow!("{e:#}")));
            return Err(e);
        }
    };

    let mut slots = SlotTracker::new(fps);
    let mut result = Ok(());
    loop {
        let active = control.active_time();
        if let Some(slot) = slots.slot(active) {
            match source.grab() {
                Ok(frame) => {
                    if let Err(e) = handle.push_owned(frame.data, slot as i64, Some(1)) {
                        result = Err(anyhow::anyhow!("encoder stopped: {e}"));
                        break;
                    }
                    frames.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    result = Err(e.context(format!("capturing \"{}\"", source.name())));
                    break;
                }
            }
        }
        let wait = slots.next_deadline().saturating_sub(control.active_time());
        if !control.wait(wait.max(Duration::from_millis(1))) {
            break;
        }
    }
    handle.finish();
    scheduler.wait().map_err(anyhow::Error::from)?;
    result?;
    match live {
        Some(live) => chunk_files(&live),
        None => Ok(vec![scratch]),
    }
}

fn chunk_files(live: &LivePaths) -> anyhow::Result<Vec<PathBuf>> {
    if let Ok(text) = std::fs::read_to_string(&live.list) {
        let files: Vec<_> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let path = PathBuf::from(line.trim());
                if path.is_absolute() {
                    path
                } else {
                    live.dir.join(path)
                }
            })
            .collect();
        if !files.is_empty() {
            return Ok(files);
        }
    }
    let mut files = std::fs::read_dir(&live.dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "mkv"))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

pub(super) enum StartFailure {
    Cancelled,
    Message(String),
}

impl From<StartFailure> for anyhow::Error {
    fn from(failure: StartFailure) -> Self {
        match failure {
            StartFailure::Cancelled => RecordingStartCancelled.into(),
            StartFailure::Message(message) => {
                anyhow::anyhow!("could not start recording: {message}")
            }
        }
    }
}

pub(super) enum SourceReady {
    Ready(String, (u32, u32)),
    Failed(String),
    Exited,
    TimedOut,
    Cancelled,
}

pub(super) fn wait_for_source(
    ready: &SourceReadyReceiver,
    cancel: &AtomicBool,
    timeout: Duration,
    poll_interval: Duration,
) -> SourceReady {
    let deadline = Instant::now() + timeout;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return SourceReady::Cancelled;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return SourceReady::TimedOut;
        }
        match ready.recv_timeout(remaining.min(poll_interval)) {
            Ok(Ok((name, size))) => return SourceReady::Ready(name, size),
            Ok(Err(error)) => return SourceReady::Failed(format!("{error:#}")),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return SourceReady::Exited;
            }
        }
    }
}
