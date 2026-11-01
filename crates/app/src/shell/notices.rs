use super::{recovery_links, App};
use crate::adapters::clipboard_worker::Purpose;
use crate::logic::notices::{notice_summary, NoticeAction, NoticeLevel};
use eframe::egui;
use mediaa_media::error::{MediaErrorKind, RecoveryAction};
use rust_i18n::t;
impl App {
    pub fn error(&mut self, msg: impl Into<String>) {
        let detail = msg.into().trim().to_string();
        let summary = notice_summary(&detail, &t!("notice.error"));
        self.notices
            .push(NoticeLevel::Error, summary, detail, None, Vec::new(), false);
    }

    pub fn error_with_kind(&mut self, kind: MediaErrorKind, msg: impl Into<String>) {
        let mut detail = msg.into().trim().to_string();
        let summary = notice_summary(&detail, &t!("notice.error"));
        if let Some(action) = kind.recovery_action() {
            let hint = match action {
                RecoveryAction::Retry => t!("err.hint_retry"),
                RecoveryAction::ChooseSource => t!("err.hint_choose_source"),
                RecoveryAction::FixPermissions => t!("err.hint_permissions"),
                RecoveryAction::FreeSpace => t!("err.hint_free_space"),
                RecoveryAction::Wait => t!("err.hint_wait"),
                RecoveryAction::UpdateApp => t!("err.hint_update"),
            };
            detail.push_str("\n\n");
            detail.push_str(&hint);
        }
        self.notices.push(
            NoticeLevel::Error,
            summary,
            detail,
            None,
            Vec::new(),
            kind.requires_attention(),
        );
    }

    pub fn warning(&mut self, msg: impl Into<String>) {
        let detail = msg.into().trim().to_string();
        let summary = notice_summary(&detail, &t!("notice.warning"));
        self.notices.push(
            NoticeLevel::Warning,
            summary,
            detail,
            None,
            Vec::new(),
            false,
        );
    }

    pub(super) fn handle_notice_action(&mut self, ctx: &egui::Context, action: NoticeAction) {
        match action {
            NoticeAction::CopyLog(log_ref) => {
                if let Err(error) = self.clipboard.handle().copy_text(log_ref, Purpose::Silent) {
                    log::warn!("copying notice log id failed: {error:#}");
                }
            }
            NoticeAction::CopyFiles(files) => {
                let links = recovery_links(&files);
                if let Err(error) = self.clipboard.handle().copy_text(links, Purpose::Silent) {
                    log::warn!("copying notice file links failed: {error:#}");
                }
            }
            NoticeAction::CopyFile(file) => {
                if let Err(error) = self
                    .clipboard
                    .handle()
                    .copy_text(mediaa_core::library::file_url(&file), Purpose::Silent)
                {
                    log::warn!("copying notice file link failed: {error:#}");
                }
            }
            NoticeAction::RevealFile(file) => self.reveal_path_from_notice(ctx, file),
        }
    }

    pub fn info(&mut self, msg: impl Into<String>) {
        let detail = msg.into().trim().to_string();
        let summary = notice_summary(&detail, &t!("notice.info"));
        self.notices
            .push(NoticeLevel::Info, summary, detail, None, Vec::new(), false);
    }

    pub fn success(&mut self, msg: impl Into<String>) {
        let detail = msg.into().trim().to_string();
        let summary = notice_summary(&detail, &t!("notice.success"));
        self.notices.push(
            NoticeLevel::Success,
            summary,
            detail,
            None,
            Vec::new(),
            false,
        );
    }
}

impl App {
    pub(crate) fn push_notice(&mut self, level: NoticeLevel, text: String) {
        match level {
            NoticeLevel::Error => self.error(text),
            NoticeLevel::Warning => self.warning(text),
            NoticeLevel::Info => self.info(text),
            NoticeLevel::Success => self.success(text),
        }
    }
}
