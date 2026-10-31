use super::*;
use crate::error::classify_media_error;

pub(super) fn concat_chunks(
    files: &[PathBuf],
    output: &Path,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    anyhow::ensure!(files.len() > 1, "at least two MKV chunks are required");
    let list = output.with_extension("concat.txt");
    let mut entries = String::new();
    for file in files {
        anyhow::ensure!(
            file.is_file() && std::fs::metadata(file).is_ok_and(|metadata| metadata.len() > 0),
            "recorded chunk is missing or empty: {}",
            file.display()
        );
        entries.push_str(&format!("file '{}'\n", concat_path(file)));
    }
    std::fs::write(&list, entries)
        .map_err(|error| anyhow::anyhow!("write concat list {}: {error}", list.display()))?;
    let input = Input::from(path_arg(&list))
        .set_format("concat")
        .set_format_opt("safe", "0");
    let result = FfmpegContext::builder()
        .input(input)
        .output(
            Output::from(path_arg(output))
                .set_format("matroska")
                .add_stream_map_with_copy("0:v"),
        )
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|context| crate::job::run(context, None, &mut |_| {}, cancel));
    let _ = std::fs::remove_file(&list);
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

fn concat_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "'\\''")
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(super) fn finish_session(
    session: RecordingSession,
    action: StopAction,
    cancel: Arc<AtomicBool>,
) {
    let events = session.events_tx.clone();
    let work_dir = session.work_dir.clone();
    let (event, keep_work_dir) = if action == StopAction::Save {
        match session.finalize(&cancel) {
            Ok(output) => (SessionEvent::Finished(output.paths), output.keep_work_dir),
            Err(failure) if crate::job::is_cancelled(&failure.error) => {
                (SessionEvent::Cancelled, failure.keep_work_dir)
            }
            Err(failure) => {
                let kind = classify_media_error(&failure.error);
                let keep_work_dir = failure.keep_work_dir
                    || failure
                        .recovery_files
                        .iter()
                        .any(|path| path.starts_with(&work_dir));
                (
                    SessionEvent::Failed(SessionFailure {
                        error: format!("{:#}", failure.error),
                        kind,
                        recovery_files: failure.recovery_files,
                        log_ref: None,
                    }),
                    keep_work_dir,
                )
            }
        }
    } else {
        session.discard();
        (SessionEvent::Cancelled, false)
    };
    if !keep_work_dir {
        let _ = std::fs::remove_dir_all(&work_dir);
    }
    let _ = events.send(event);
}

pub(super) fn preserve_recordings(
    videos: &[RecordedVideo],
    out_dir: &Path,
    title: &str,
    started: chrono::DateTime<chrono::Local>,
) -> Vec<PathBuf> {
    let mut kept = Vec::new();
    for (index, video) in videos.iter().enumerate() {
        let sources: Vec<_> = video.files.iter().filter(|path| path.is_file()).collect();
        for (part, source) in sources.iter().enumerate() {
            let title = linked_title(title, index, videos.len(), &video.name, false);
            let suffix = if sources.len() == 1 {
                " (source)".to_string()
            } else {
                format!(" (source part {} of {})", part + 1, sources.len())
            };
            let target = naming::media_path(out_dir, &format!("{title}{suffix}"), started, "mkv");
            if let Some(path) = preserve_file(source, &target) {
                kept.push(path);
            }
        }
    }
    kept
}

fn preserve_file(source: &Path, target: &Path) -> Option<PathBuf> {
    match move_recovery_file(source, target) {
        Ok(()) => Some(target.to_path_buf()),
        Err(error) => {
            log::warn!("keep source recording {}: {error:#}", source.display());
            Some(source.to_path_buf())
        }
    }
}
