pub use mediaa_core::notices::{notice_summary, NoticeLevel};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const NOTICE_WIDTH: f32 = 460.0;
pub const NOTICE_FADE_IN: Duration = Duration::from_millis(180);
pub const NOTICE_ANIMATION_FRAME: Duration = Duration::from_millis(16);
pub const NOTICE_IMPORTANT_LIFETIME: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct AppNotice {
    pub id: u64,
    pub level: NoticeLevel,
    pub summary: String,
    pub detail: String,
    pub log_ref: String,
    pub files: Vec<PathBuf>,
    pub expanded: bool,
    pub important: bool,
    pub opened_at: Instant,
    pub closing_at: Option<Instant>,
    pub expires_at: Instant,
    pub hovered: bool,
    pub paused_remaining: Option<Duration>,
}

#[derive(Clone)]
pub struct Notices {
    pub items: Vec<AppNotice>,
    next_id: u64,
    pub enabled: bool,
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeAction {
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
    pub fn with_enabled(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.items.clear();
            self.visible = false;
        }
        self.enabled = enabled;
    }

    pub fn push(
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
            NoticeLevel::Info | NoticeLevel::Success => log::info!("[{log_ref}] {detail}"),
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

    pub fn begin_close(&mut self, id: u64) {
        if let Some(notice) = self.items.iter_mut().find(|notice| notice.id == id) {
            notice.closing_at.get_or_insert_with(Instant::now);
        }
    }

    pub fn expire(&mut self, now: Instant) {
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
    fn notices_keep_ids_and_limit_transient_items() {
        let mut notices = Notices::default();
        notices.push(
            NoticeLevel::Info,
            "one".into(),
            String::new(),
            None,
            Vec::new(),
            false,
        );
        notices.push(
            NoticeLevel::Error,
            "two".into(),
            String::new(),
            None,
            Vec::new(),
            true,
        );
        assert_eq!(notices.items[0].id, 1);
        assert_eq!(notices.items[1].id, 2);
        for index in 0..8 {
            notices.push(
                NoticeLevel::Info,
                format!("n{index}"),
                String::new(),
                None,
                Vec::new(),
                false,
            );
        }
        assert_eq!(notices.items.len(), 5);
        assert!(notices.items.iter().any(|notice| notice.important));
    }
}
