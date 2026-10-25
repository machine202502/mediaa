use super::*;

impl App {
    fn show_recovery(&mut self, recovery: RecoveryReport) {
        if !recovery.files.is_empty() {
            self.lib.files_changed(&recovery.files);
            let library = pages::library::library_dir(&self.settings);
            let mut detail = t!(
                "toast.recovered",
                count = recovery.files.len(),
                folder = library.display()
            )
            .into_owned();
            if !recovery.failures.is_empty() {
                detail.push_str("\n\n");
                detail.push_str(&recovery.failures.join("\n"));
            }
            self.notices.push(
                NoticeLevel::Warning,
                t!("toast.recovered_summary", count = recovery.files.len()).into_owned(),
                detail,
                Some("recording-recovery".into()),
                recovery.files,
                true,
            );
        } else if !recovery.failures.is_empty() {
            self.notices.push(
                NoticeLevel::Error,
                t!("toast.recovery_failed").into_owned(),
                recovery.failures.join("\n"),
                Some("recording-recovery".into()),
                Vec::new(),
                true,
            );
        }
    }

    pub(super) fn start_recovery(&mut self) {
        if self.recovery_rx.is_some() {
            return;
        }
        let work = paths::work_dir();
        let has_session = std::fs::read_dir(&work).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        });
        if !has_session {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let media = pages::library::library_dir(&self.settings);
        match self
            .tasks
            .spawn(TaskKind::Recovery, "recording-recovery", None, move || {
                let _ = tx.send(recover_incomplete_sessions(&work, &media));
            }) {
            Ok(task) => {
                self.recovery_rx = Some(rx);
                self.recovery_task = Some(task);
            }
            Err(error) => self.error(t!("toast.recovery_failed_detail", error = error)),
        }
    }

    pub(super) fn poll_recovery(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.recovery_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(recovery) => {
                self.recovery_rx = None;
                if let Some(task) = self.recovery_task.take() {
                    self.tasks.finish(task);
                }
                self.show_recovery(recovery);
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100))
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.recovery_rx = None;
                if let Some(task) = self.recovery_task.take() {
                    self.tasks.finish(task);
                }
                self.error(t!("toast.recovery_failed"));
            }
        }
    }
}
