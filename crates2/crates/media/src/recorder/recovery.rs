use super::*;

#[derive(Debug, Default)]
pub struct RecoveryReport {
    pub files: Vec<PathBuf>,
    pub failures: Vec<String>,
}

struct RecoveryCandidate {
    source: PathBuf,
    title: String,
}

fn numbered_video(name: &str) -> Option<usize> {
    let stem = name.strip_suffix(".mkv")?;
    stem.strip_prefix("video-")
        .and_then(|value| value.parse::<usize>().ok())
}

fn sorted_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|entry| entry.file_type().ok()?.is_file().then(|| entry.path()))
        .collect();
    files.sort();
    files
}

fn usable_file(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

fn recovery_candidates(session: &Path) -> Vec<RecoveryCandidate> {
    let files = sorted_files(session);
    let mut video_indices = std::collections::BTreeSet::new();
    for file in &files {
        if let Some(index) = file
            .file_name()
            .and_then(|name| numbered_video(&name.to_string_lossy()))
        {
            video_indices.insert(index);
        }
    }
    let mut candidates = Vec::new();
    for index in video_indices {
        let scratch = session.join(format!("video-{index}.mkv"));
        if usable_file(&scratch) {
            candidates.push(RecoveryCandidate {
                source: scratch,
                title: format!("Recovered recording video {}", index + 1),
            });
        }
    }
    for (index, source) in files
        .into_iter()
        .filter(|path| path.extension().is_some_and(|value| value == "wav") && usable_file(path))
        .enumerate()
    {
        candidates.push(RecoveryCandidate {
            source,
            title: format!("Recovered audio {}", index + 1),
        });
    }
    candidates
}

pub(super) fn move_recovery_file(source: &Path, target: &Path) -> anyhow::Result<()> {
    if std::fs::hard_link(source, target).is_ok() {
        let _ = std::fs::remove_file(source);
        return Ok(());
    }
    let pending = AtomicFile::new(target)?;
    std::fs::copy(source, pending.path())?;
    pending.commit()?;
    let _ = std::fs::remove_file(source);
    Ok(())
}

pub fn recover_incomplete_sessions(work_root: &Path, out_dir: &Path) -> RecoveryReport {
    let _ = std::fs::create_dir_all(out_dir);
    let Ok(entries) = std::fs::read_dir(work_root) else {
        return RecoveryReport::default();
    };
    let mut sessions: Vec<_> = entries
        .flatten()
        .filter_map(|entry| entry.file_type().ok()?.is_dir().then(|| entry.path()))
        .collect();
    sessions.sort();
    let mut report = RecoveryReport::default();
    for session in sessions {
        let candidates = recovery_candidates(&session);
        let at = session
            .metadata()
            .and_then(|metadata| metadata.modified())
            .map(chrono::DateTime::<chrono::Local>::from)
            .unwrap_or_else(|_| chrono::Local::now());
        let mut failed = false;
        for candidate in candidates {
            let extension = candidate
                .source
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("mkv");
            let target = naming::media_path(out_dir, &candidate.title, at, extension);
            match move_recovery_file(&candidate.source, &target) {
                Ok(()) => report.files.push(target),
                Err(error) => {
                    failed = true;
                    report
                        .failures
                        .push(format!("{}: {error:#}", candidate.source.display()));
                }
            }
        }
        if !failed {
            if let Err(error) = std::fs::remove_dir_all(&session) {
                report.failures.push(format!(
                    "{}: cannot clean recovered session: {error}",
                    session.display()
                ));
            }
        }
    }
    report
}
