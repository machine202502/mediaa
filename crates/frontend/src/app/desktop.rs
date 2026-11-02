use super::state::{DesktopAction, DesktopEvent};
use super::*;

impl App {
    pub fn open_path(&mut self, ctx: &egui::Context, path: &std::path::Path) {
        self.spawn_desktop(ctx, DesktopAction::Open(path.to_path_buf()), true);
    }

    pub(crate) fn reveal_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.spawn_desktop(ctx, DesktopAction::Reveal(path), true);
    }

    pub(super) fn reveal_path_from_notice(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.spawn_desktop(ctx, DesktopAction::Reveal(path), false);
    }

    pub(crate) fn rename_media(&mut self, ctx: &egui::Context, path: PathBuf, title: String) {
        let new = match mediaa_core::naming::rename_target(&path, &title) {
            Ok(path) => path,
            Err(error) => {
                self.error(format!("{error:#}"));
                return;
            }
        };
        if new == path {
            return;
        }
        let rename = mediaa_protocol::backend::LibraryRenameWire {
            from: path.to_string_lossy().into_owned(),
            to: new.to_string_lossy().into_owned(),
            title: title.clone(),
        };
        self.send_sync_command(
            mediaa_protocol::backend::LIBRARY_RENAME,
            mediaa_protocol::backend::encode_library_rename(&rename),
        );
        #[cfg(test)]
        if let Err(error) = mediaa_core::naming::rename(&path, &title) {
            self.error(format!("{error:#}"));
            return;
        }
        if let Err(error) = mediaa_media::preview_cache::relink_proxies(&path, &new) {
            log::warn!(
                "preview relink {} -> {}: {error:#}",
                path.display(),
                new.display()
            );
        }
        self.thumbs.relink(&path, &new);
        self.lib.rename_selected(&path, &new);
        self.send_library_renamed(&path, &new);
        self.success(t!(
            "lib.renamed",
            name = new.file_name().unwrap_or_default().to_string_lossy()
        ));
        ctx.request_repaint();
    }

    fn spawn_desktop(&mut self, ctx: &egui::Context, action: DesktopAction, notify_errors: bool) {
        let task_name = "external-open";
        let worker_action = action.clone();
        let events = self.desktop_events_tx.clone();
        let worker_ctx = ctx.clone();
        match crate::features::jobs::Job::spawn(task_name, std::sync::Arc::default(), move || {
            let result = execute(&worker_action);
            let _ = events.send(DesktopEvent {
                action: worker_action,
                result,
                notify_errors,
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
                        if event.notify_errors {
                            self.error(error);
                        } else {
                            log::warn!("desktop action from notice failed: {error}");
                        }
                    }
                }
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
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rename_target_is_computed_without_touching_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path =
            mediaa_core::naming::media_path(dir.path(), "Original", chrono::Local::now(), "png");
        std::fs::write(&path, b"image").unwrap();
        let renamed = mediaa_core::naming::rename_target(&path, "Renamed").unwrap();
        assert!(path.exists());
        assert!(!renamed.exists());
        assert!(renamed.to_string_lossy().contains("Renamed"));
    }
}
