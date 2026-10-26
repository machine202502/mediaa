use super::*;

impl App {
    fn show_recovery(&mut self, recovery: RecoveryReport) {
        if !recovery.files.is_empty() {
            self.lib.files_changed(&recovery.files);
            let library = features::library::library_dir(&self.settings);
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
        if self.recovery_job.is_some() {
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
        let media = features::library::library_dir(&self.settings);
        match crate::jobs::Job::spawn("recording-recovery", std::sync::Arc::default(), move || {
            recover_incomplete_sessions(&work, &media)
        }) {
            Ok(job) => self.recovery_job = Some(job),
            Err(error) => self.error(t!("toast.recovery_failed_detail", error = error)),
        }
    }

    pub(super) fn poll_recovery(&mut self, ctx: &egui::Context) {
        let Some(job) = self.recovery_job.as_mut() else {
            return;
        };
        match job.poll() {
            Some(Ok(recovery)) => {
                self.recovery_job = None;
                self.show_recovery(recovery);
            }
            None => ctx.request_repaint_after(Duration::from_millis(100)),
            Some(Err(error)) => {
                self.recovery_job = None;
                log::warn!("recording recovery stopped: {error:#}");
                self.error(t!("toast.recovery_failed"));
            }
        }
    }
}
