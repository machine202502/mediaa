use std::path::PathBuf;
use std::time::{Duration, Instant};

pub(crate) const NOTICE_SUMMARY_CHARS: usize = 72;
pub(crate) const NOTICE_WIDTH: f32 = 460.0;
pub(crate) const NOTICE_FADE_IN: Duration = Duration::from_millis(180);
pub(crate) const NOTICE_ANIMATION_FRAME: Duration = Duration::from_millis(16);
pub(crate) const NOTICE_IMPORTANT_LIFETIME: Duration = Duration::from_secs(30);

fn one_line(message: impl Into<String>) -> String {
    message
        .into()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
pub(crate) fn notice_summary(message: &str, fallback: &str) -> String {
    let message = one_line(message);
    if message.chars().count() <= NOTICE_SUMMARY_CHARS {
        return message;
    }
    let prefix = message
        .char_indices()
        .find(|(_, ch)| matches!(ch, ':' | '—'))
        .map(|(index, ch)| message[..index + ch.len_utf8()].trim())
        .filter(|prefix| prefix.chars().count() >= 8);
    prefix.unwrap_or(fallback).to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoticeLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub(crate) struct AppNotice {
    pub(crate) id: u64,
    pub(crate) level: NoticeLevel,
    pub(crate) summary: String,
    pub(crate) detail: String,
    pub(crate) log_ref: String,
    pub(crate) files: Vec<PathBuf>,
    pub(crate) expanded: bool,
    pub(crate) important: bool,
    pub(crate) opened_at: Instant,
    pub(crate) closing_at: Option<Instant>,
    pub(crate) expires_at: Instant,
    pub(crate) hovered: bool,
    pub(crate) paused_remaining: Option<Duration>,
}

#[derive(Clone)]
pub(crate) struct Notices {
    pub(crate) items: Vec<AppNotice>,
    next_id: u64,
    pub(crate) enabled: bool,
    pub(crate) visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoticeAction {
    CopyLog(String),
    CopyFiles(Vec<PathBuf>),
    CopyFile(PathBuf),
    RevealFile(PathBuf),
}

impl Default for Notices {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            next_id: 0,
            enabled: true,
            visible: false,
        }
    }
}

impl Notices {
    pub(crate) fn with_enabled(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.items.clear();
            self.visible = false;
        }
        self.enabled = enabled;
    }

    pub(crate) fn push(
        &mut self,
        level: NoticeLevel,
        summary: String,
        detail: String,
        log_ref: Option<String>,
        files: Vec<PathBuf>,
        important: bool,
    ) {
        self.next_id += 1;
        let id = self.next_id;
        let log_ref = log_ref.unwrap_or_else(|| format!("notice-{id:04}"));
        match level {
            NoticeLevel::Info => log::info!("[{log_ref}] {detail}"),
            NoticeLevel::Success => log::info!("[{log_ref}] {detail}"),
            NoticeLevel::Warning => log::warn!("[{log_ref}] {detail}"),
            NoticeLevel::Error => log::error!("[{log_ref}] {detail}"),
        }
        if !self.enabled {
            return;
        }
        let lifetime = if important {
            NOTICE_IMPORTANT_LIFETIME
        } else {
            match level {
                NoticeLevel::Info | NoticeLevel::Success => Duration::from_secs(5),
                NoticeLevel::Warning => Duration::from_secs(8),
                NoticeLevel::Error => Duration::from_secs(10),
            }
        };
        let now = Instant::now();
        self.items.push(AppNotice {
            id,
            level,
            summary,
            detail,
            log_ref,
            files,
            expanded: false,
            important,
            opened_at: now,
            closing_at: None,
            expires_at: now + lifetime,
            hovered: false,
            paused_remaining: None,
        });
        while self.items.len() > 5 {
            let index = self
                .items
                .iter()
                .position(|notice| !notice.important)
                .unwrap_or(0);
            self.items.remove(index);
        }
    }

    pub(crate) fn begin_close(&mut self, id: u64) {
        if let Some(notice) = self.items.iter_mut().find(|notice| notice.id == id) {
            notice.closing_at.get_or_insert_with(Instant::now);
        }
    }

    pub(crate) fn expire(&mut self, now: Instant) {
        for notice in &mut self.items {
            if notice.hovered {
                notice
                    .paused_remaining
                    .get_or_insert(notice.expires_at.saturating_duration_since(now));
                continue;
            }
            if let Some(remaining) = notice.paused_remaining.take() {
                notice.expires_at = now + remaining;
            }
            if now >= notice.expires_at {
                notice.closing_at.get_or_insert(now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_text_stays_on_one_short_line() {
        let detail = format!("Ошибка:\n{}", "C:\\very\\long\\path\\".repeat(20));
        let summary = notice_summary(&detail, "Ошибка");
        assert_eq!(summary, "Ошибка");
        assert!(!summary.ends_with('…'));
        assert_eq!(
            notice_summary(
                "Не удалось сохранить запись: очень длинное техническое описание ошибки с путём и цепочкой повторов",
                "Ошибка"
            ),
            "Не удалось сохранить запись:"
        );
        assert_eq!(notice_summary("Коротко", "Ошибка"), "Коротко");

        let mut notices = Notices::default();
        notices.push(
            NoticeLevel::Error,
            summary,
            detail.clone(),
            None,
            Vec::new(),
            true,
        );
        notices.push(
            NoticeLevel::Warning,
            "Предупреждение".into(),
            "Полный текст".into(),
            Some("webp-test".into()),
            Vec::new(),
            false,
        );
        assert_eq!(notices.items[0].id, 1);
        assert_eq!(notices.items[1].id, 2);
        assert_eq!(notices.items[0].detail, detail);
        assert_eq!(notices.items[1].log_ref, "webp-test");
    }

    #[test]
    fn important_notice_is_not_evicted_by_new_transient_toasts() {
        let mut notices = Notices::default();
        notices.push(
            NoticeLevel::Warning,
            "Recovery".into(),
            "Files".into(),
            None,
            Vec::new(),
            true,
        );
        for index in 0..8 {
            notices.push(
                NoticeLevel::Info,
                format!("Message {index}"),
                String::new(),
                None,
                Vec::new(),
                false,
            );
        }
        assert_eq!(notices.items.len(), 5);
        assert!(notices
            .items
            .iter()
            .any(|notice| notice.summary == "Recovery"));
    }
}
