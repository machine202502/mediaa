use super::*;

impl App {
    pub(super) fn show_recovery(&mut self, recovery: RecoveryReport) {
        if !recovery.files.is_empty() {
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
}
