use super::state::{DesktopAction, DesktopEvent};
use super::*;

impl App {
    pub fn open_path(&mut self, ctx: &egui::Context, path: &std::path::Path) {
        self.spawn_desktop(ctx, DesktopAction::Open(path.to_path_buf()));
    }

    pub(crate) fn reveal_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.spawn_desktop(ctx, DesktopAction::Reveal(path));
    }

    pub(crate) fn rename_media(&mut self, ctx: &egui::Context, path: PathBuf, title: String) {
        self.spawn_desktop(ctx, DesktopAction::Rename { path, title });
    }

    fn spawn_desktop(&mut self, ctx: &egui::Context, action: DesktopAction) {
        let task_name = match &action {
            DesktopAction::Open(_) | DesktopAction::Reveal(_) => "external-open",
            DesktopAction::Rename { .. } => "rename-media",
        };
        let worker_action = action.clone();
        let events = self.desktop_events_tx.clone();
        let worker_ctx = ctx.clone();
        match crate::jobs::Job::spawn(task_name, std::sync::Arc::default(), move || {
            let result = execute(&worker_action);
            let _ = events.send(DesktopEvent {
                action: worker_action,
                result,
            });
            worker_ctx.request_repaint();
        }) {
            Ok(job) => self.desktop_jobs.push(job),
            Err(error) => self.error(format!("{error:#}")),
        }
    }

    pub(super) fn poll_desktop(&mut self) {
        while let Ok(event) = self.desktop_events.try_recv() {
            match event.action {
                DesktopAction::Open(_) | DesktopAction::Reveal(_) => {
                    if let Err(error) = event.result {
                        self.error(error);
                    }
                }
                DesktopAction::Rename { path, .. } => match event.result {
                    Ok(Some(new)) if new != path => {
                        let proxy = mediaa_media::preview_cache::proxy_path(&path);
                        self.thumbs.forget(&path, proxy);
                        self.lib.files_changed(&[path, new.clone()]);
                        self.success(t!(
                            "lib.renamed",
                            name = new.file_name().unwrap_or_default().to_string_lossy()
                        ));
                    }
                    Ok(_) => {}
                    Err(error) => self.error(error),
                },
            }
        }
        self.desktop_jobs.retain_mut(|job| {
            if let Some(result) = job.poll() {
                if let Err(error) = result {
                    log::warn!("desktop worker stopped: {error:#}");
                }
                false
            } else {
                true
            }
        });
    }
}

fn execute(action: &DesktopAction) -> Result<Option<PathBuf>, String> {
    match action {
        DesktopAction::Open(path) => files::open(path)
            .map(|()| None)
            .map_err(|error| format!("{error:#}")),
        DesktopAction::Reveal(path) => files::reveal(path)
            .map(|()| None)
            .map_err(|error| format!("{error:#}")),
        DesktopAction::Rename { path, title } => naming::rename(path, title)
            .map(Some)
            .map_err(|error| format!("{error:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{execute, DesktopAction};

    #[test]
    fn rename_returns_the_published_path() {
        let dir = tempfile::tempdir().unwrap();
        let path =
            mediaa_core::naming::media_path(dir.path(), "Original", chrono::Local::now(), "png");
        std::fs::write(&path, b"image").unwrap();
        let renamed = execute(&DesktopAction::Rename {
            path: path.clone(),
            title: "Renamed".into(),
        })
        .unwrap()
        .unwrap();
        assert!(!path.exists());
        assert!(renamed.exists());
        assert!(renamed.to_string_lossy().contains("Renamed"));
    }
}
