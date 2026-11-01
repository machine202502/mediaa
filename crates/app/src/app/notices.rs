use super::{recovery_links, App};
use crate::services::clipboard::Purpose;
use crate::theme;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa_media::error::{MediaErrorKind, RecoveryAction};
use rust_i18n::t;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const NOTICE_SUMMARY_CHARS: usize = 72;
const NOTICE_WIDTH: f32 = 460.0;
const NOTICE_FADE_IN: Duration = Duration::from_millis(180);
const NOTICE_ANIMATION_FRAME: Duration = Duration::from_millis(16);
const NOTICE_IMPORTANT_LIFETIME: Duration = Duration::from_secs(30);

fn one_line(message: impl Into<String>) -> String {
    message
        .into()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn notice_opacity(notice: &AppNotice, now: Instant) -> (f32, bool) {
    if notice.closing_at.is_some() {
        return (0.0, true);
    }
    let elapsed = now
        .saturating_duration_since(notice.opened_at)
        .as_secs_f32();
    let progress = (elapsed / NOTICE_FADE_IN.as_secs_f32()).clamp(0.0, 1.0);
    (smoothstep(progress), false)
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
    enabled: bool,
    visible: bool,
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

    fn begin_close(&mut self, id: u64) {
        if let Some(notice) = self.items.iter_mut().find(|notice| notice.id == id) {
            notice.closing_at.get_or_insert_with(Instant::now);
        }
    }

    fn expire(&mut self, now: Instant) {
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

    pub(crate) fn show(&mut self, ctx: &egui::Context) -> Option<NoticeAction> {
        self.show_with(ctx, true)
    }

    pub(crate) fn show_blocked(&mut self, ctx: &egui::Context) -> Option<NoticeAction> {
        self.show_with(ctx, false)
    }

    fn show_with(&mut self, ctx: &egui::Context, interactive: bool) -> Option<NoticeAction> {
        if !self.enabled {
            return None;
        }
        let before = self.items.len();
        self.items.retain(|notice| notice.closing_at.is_none());
        if self.items.len() != before {
            ctx.request_repaint();
        }
        if self.items.is_empty() {
            self.visible = false;
            return None;
        }
        let first_show = !self.visible;
        self.visible = true;
        let now = Instant::now();
        let mut close = None;
        let mut copy_log = None;
        let mut copy_files = None;
        let mut copy_file = None;
        let mut reveal_file = None;
        let mut remove = Vec::new();
        self.expire(now);
        egui::Area::new(egui::Id::new("app-notices"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
            .order(egui::Order::Foreground)
            .sizing_pass(first_show)
            .show(ctx, |ui| {
                if !interactive {
                    ui.disable();
                }
                let p = theme::palette(ui);
                ui.set_max_width(NOTICE_WIDTH);
                ui.spacing_mut().item_spacing.y = 6.0;
                for notice in &mut self.items {
                    let (opacity, finished) = notice_opacity(notice, now);
                    if finished {
                        remove.push(notice.id);
                        continue;
                    }
                    let color = match notice.level {
                        NoticeLevel::Info => p.accent,
                        NoticeLevel::Success => p.success,
                        NoticeLevel::Warning => p.warning,
                        NoticeLevel::Error => p.danger,
                    };
                    let icon = match notice.level {
                        NoticeLevel::Info => icon::INFO,
                        NoticeLevel::Success => icon::CHECK_CIRCLE,
                        NoticeLevel::Warning => icon::WARNING,
                        NoticeLevel::Error => icon::WARNING_OCTAGON,
                    };
                    let width = NOTICE_WIDTH;
                    let mut toggle = false;
                    let mut card_close = false;
                    let mut card_copy_log = false;
                    let mut card_copy_files = false;
                    let mut card_copy_file = None;
                    let mut card_reveal_file = None;
                    let response = ui
                        .scope(|ui| {
                            ui.set_opacity(opacity);
                            egui::Frame::new()
                                .fill(p.card)
                                .stroke(egui::Stroke::NONE)
                                .corner_radius(egui::CornerRadius::same(4))
                                .inner_margin(egui::Margin::same(10))
                                .show(ui, |ui| {
                                    ui.set_width(width - 20.0);
                                    ui.horizontal(|ui| {
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(20.0, 22.0),
                                            egui::Layout::centered_and_justified(
                                                egui::Direction::LeftToRight,
                                            ),
                                            |ui| {
                                                ui.label(
                                                    egui::RichText::new(icon)
                                                        .color(color)
                                                        .size(16.0),
                                                );
                                            },
                                        );
                                        let text_width = (ui.available_width() - 54.0).max(120.0);
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(text_width, 22.0),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                let summary = ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(&notice.summary)
                                                            .size(13.0),
                                                    )
                                                    .wrap()
                                                    .sense(egui::Sense::click()),
                                                );
                                                if summary.clicked() {
                                                    toggle = true;
                                                }
                                            },
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui
                                                    .add(
                                                        egui::Button::new(
                                                            egui::RichText::new(icon::X)
                                                                .color(p.text_weak)
                                                                .size(13.0),
                                                        )
                                                        .frame(false),
                                                    )
                                                    .clicked()
                                                {
                                                    card_close = true;
                                                }
                                                if ui
                                                    .add(
                                                        egui::Button::new(
                                                            egui::RichText::new(
                                                                if notice.expanded {
                                                                    icon::CARET_UP
                                                                } else {
                                                                    icon::CARET_DOWN
                                                                },
                                                            )
                                                            .color(p.text_weak)
                                                            .size(14.0),
                                                        )
                                                        .frame(false),
                                                    )
                                                    .clicked()
                                                {
                                                    toggle = true;
                                                }
                                            },
                                        );
                                    });
                                    if notice.expanded {
                                        ui.add_space(4.0);
                                        ui.separator();
                                        egui::ScrollArea::vertical()
                                            .id_salt(("notice-detail", notice.id))
                                            .max_height(260.0)
                                            .show(ui, |ui| {
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(&notice.detail)
                                                            .size(12.0),
                                                    )
                                                    .wrap()
                                                    .selectable(false),
                                                );
                                            });
                                        ui.horizontal_wrapped(|ui| {
                                            let log_id = ui
                                                .add(
                                                    egui::Label::new(
                                                        egui::RichText::new(format!(
                                                            "{} {}",
                                                            t!("notice.log_id"),
                                                            notice.log_ref
                                                        ))
                                                        .monospace()
                                                        .color(p.text_weak)
                                                        .size(10.5),
                                                    )
                                                    .sense(egui::Sense::click()),
                                                )
                                                .on_hover_text(t!("notice.copy_log_id"));
                                            if log_id.clicked() {
                                                card_copy_log = true;
                                            }
                                            if ui
                                                .small_button(t!("notice.copy"))
                                                .on_hover_text(t!("notice.copy_log_id"))
                                                .clicked()
                                            {
                                                card_copy_log = true;
                                            }
                                        });
                                        if !notice.files.is_empty() {
                                            ui.add_space(4.0);
                                            ui.separator();
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new(t!(
                                                        "notice.mkv_links",
                                                        count = notice.files.len()
                                                    ))
                                                    .color(p.accent)
                                                    .size(10.5),
                                                );
                                                if ui
                                                    .small_button(t!("notice.copy_links"))
                                                    .on_hover_text(t!("toast.recovery_hint"))
                                                    .clicked()
                                                {
                                                    card_copy_files = true;
                                                }
                                            });
                                            egui::ScrollArea::vertical()
                                                .id_salt(("notice-files", notice.id))
                                                .max_height(130.0)
                                                .show(ui, |ui| {
                                                    for (index, file) in
                                                        notice.files.iter().enumerate()
                                                    {
                                                        let name = file
                                                            .file_name()
                                                            .unwrap_or_default()
                                                            .to_string_lossy();
                                                        ui.push_id(
                                                            ("notice-file", notice.id, index),
                                                            |ui| {
                                                                ui.horizontal(|ui| {
                                                                    let actions_width = 140.0;
                                                                    let text_width = (ui
                                                                        .available_width()
                                                                        - actions_width)
                                                                        .max(100.0);
                                                                    ui.allocate_ui_with_layout(
                                                                        egui::vec2(text_width, 0.0),
                                                                        egui::Layout::left_to_right(
                                                                            egui::Align::Center,
                                                                        ),
                                                                        |ui| {
                                                                            ui.add(
                                                                        egui::Label::new(
                                                                            egui::RichText::new(
                                                                                name.as_ref(),
                                                                            )
                                                                            .size(10.5),
                                                                        )
                                                                        .wrap()
                                                                        .selectable(false),
                                                                    );
                                                                        },
                                                                    );
                                                                    if ui
                                                                        .small_button(t!(
                                                                            "notice.copy_link"
                                                                        ))
                                                                        .clicked()
                                                                    {
                                                                        card_copy_file =
                                                                            Some(file.clone());
                                                                    }
                                                                    if ui
                                                                        .small_button(t!(
                                                                            "notice.show_file"
                                                                        ))
                                                                        .clicked()
                                                                    {
                                                                        card_reveal_file =
                                                                            Some(file.clone());
                                                                    }
                                                                });
                                                            },
                                                        );
                                                    }
                                                });
                                        }
                                    }
                                })
                        })
                        .inner;
                    notice.hovered = notice.closing_at.is_none() && response.response.hovered();
                    if notice.closing_at.is_none() {
                        if card_close {
                            close = Some(notice.id);
                        } else if card_copy_log {
                            copy_log = Some(notice.log_ref.clone());
                        } else if card_copy_files {
                            copy_files = Some(notice.files.clone());
                        } else if card_copy_file.is_some() {
                            copy_file = card_copy_file;
                        } else if card_reveal_file.is_some() {
                            reveal_file = card_reveal_file;
                        } else if toggle {
                            notice.expanded = !notice.expanded;
                            if notice.expanded {
                                notice.expires_at = notice
                                    .expires_at
                                    .max(Instant::now() + Duration::from_secs(8));
                            }
                        }
                    }
                }
            });
        if let Some(id) = close {
            self.begin_close(id);
        }
        self.items.retain(|notice| !remove.contains(&notice.id));
        let now = Instant::now();
        self.expire(now);
        let mut wake = None;
        for notice in &self.items {
            if notice.closing_at.is_some()
                || now.saturating_duration_since(notice.opened_at) < NOTICE_FADE_IN
            {
                wake = Some(NOTICE_ANIMATION_FRAME);
                break;
            }
            if !notice.hovered {
                let until_expiry = notice.expires_at.saturating_duration_since(now);
                wake =
                    Some(wake.map_or(until_expiry, |current: Duration| current.min(until_expiry)));
            }
        }
        if let Some(wake) = wake {
            ctx.request_repaint_after(wake);
        }
        if let Some(log_ref) = copy_log {
            Some(NoticeAction::CopyLog(log_ref))
        } else if copy_file.is_some() {
            copy_file.map(NoticeAction::CopyFile)
        } else if reveal_file.is_some() {
            reveal_file.map(NoticeAction::RevealFile)
        } else {
            copy_files.map(NoticeAction::CopyFiles)
        }
    }
}

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

    pub(crate) fn show_notices(&mut self, ctx: &egui::Context, interactive: bool) {
        self.notices
            .set_enabled(self.settings.notifications_enabled);
        let action = if interactive {
            self.notices.show(ctx)
        } else {
            self.notices.show_blocked(ctx)
        };
        if let Some(action) = action {
            self.handle_notice_action(ctx, action);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::{NodeT, Queryable};

    #[test]
    fn disabled_notices_are_not_queued() {
        let mut notices = Notices::with_enabled(false);
        notices.push(
            NoticeLevel::Info,
            "Hidden".into(),
            "Detail".into(),
            None,
            Vec::new(),
            false,
        );
        assert!(notices.items.is_empty());
        notices.set_enabled(true);
        notices.push(
            NoticeLevel::Info,
            "Shown".into(),
            "Detail".into(),
            None,
            Vec::new(),
            false,
        );
        assert_eq!(notices.items.len(), 1);
        notices.set_enabled(false);
        assert!(notices.items.is_empty());
    }
    fn notice() -> (Notices, Instant) {
        let mut notices = Notices::default();
        notices.push(
            NoticeLevel::Info,
            "Notice".into(),
            "Detail".into(),
            None,
            Vec::new(),
            false,
        );
        let opened = notices.items[0].opened_at;
        (notices, opened)
    }

    #[test]
    fn blocked_notices_are_disabled() {
        let _ui = crate::app::test_support::ui_test_guard();
        let (mut notices, _) = notice();
        notices.items[0].expanded = true;
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, notices: &mut Notices| {
                notices.show_blocked(ui.ctx());
            },
            notices,
        );
        harness.run_steps(3);
        let copy = t!("notice.copy").to_string();
        assert!(harness.get_by_label(&copy).accesskit_node().is_disabled());
    }

    #[test]
    fn notice_fades_in_and_out() {
        let (notices, opened) = notice();
        assert_eq!(notice_opacity(&notices.items[0], opened), (0.0, false));
        let halfway = opened + NOTICE_FADE_IN / 2;
        let (opening, finished) = notice_opacity(&notices.items[0], halfway);
        assert!((0.0..1.0).contains(&opening));
        assert!(!finished);
        assert_eq!(
            notice_opacity(&notices.items[0], opened + NOTICE_FADE_IN),
            (1.0, false)
        );

        let mut notices = notices;
        notices.items[0].closing_at = Some(halfway);
        assert_eq!(
            notice_opacity(&notices.items[0], halfway + Duration::from_millis(1)),
            (0.0, true)
        );
    }

    #[test]
    fn expanded_notice_still_expires() {
        let (mut notices, opened) = notice();
        notices.items[0].expanded = true;
        notices.items[0].expires_at = opened;
        notices.expire(opened);
        assert!(notices.items[0].closing_at.is_some());
    }

    #[test]
    fn hover_pauses_the_remaining_lifetime() {
        let (mut notices, opened) = notice();
        notices.items[0].expires_at = opened + Duration::from_secs(5);
        notices.items[0].hovered = true;
        let paused_at = opened + Duration::from_secs(4);
        notices.expire(paused_at);
        assert!(notices.items[0].closing_at.is_none());
        assert_eq!(
            notices.items[0].paused_remaining,
            Some(Duration::from_secs(1))
        );

        notices.expire(paused_at + Duration::from_secs(10));
        assert!(notices.items[0].closing_at.is_none());
        notices.items[0].hovered = false;
        let resumed_at = paused_at + Duration::from_secs(10);
        notices.expire(resumed_at);
        assert_eq!(
            notices.items[0]
                .expires_at
                .saturating_duration_since(resumed_at),
            Duration::from_secs(1)
        );
        assert!(notices.items[0].closing_at.is_none());
        notices.expire(resumed_at + Duration::from_secs(1));
        assert!(notices.items[0].closing_at.is_some());
    }

    #[test]
    fn collapsed_notices_share_one_width() {
        let _ui = crate::app::test_support::ui_test_guard();
        let mut notices = Notices::default();
        for summary in ["Short", "A much longer notice summary that wraps a little"] {
            notices.push(
                NoticeLevel::Info,
                summary.into(),
                "Detail".into(),
                None,
                Vec::new(),
                false,
            );
        }
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, notices: &mut Notices| {
                notices.show(ui.ctx());
            },
            notices,
        );
        harness.run_steps(3);
        let closes: Vec<_> = harness
            .get_all_by_label(icon::X)
            .map(|node| node.rect().right())
            .collect();
        assert_eq!(closes.len(), 2);
        assert!(
            (closes[0] - closes[1]).abs() < 0.5,
            "collapsed notices have one fixed width: {closes:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn ui_regression_snapshot_notices() {
        let _ui = crate::app::test_support::ui_test_guard();
        let mut notices = Notices::default();
        notices.push(
            NoticeLevel::Success,
            "Converted".into(),
            "The result is ready in the library".into(),
            Some("convert-0001".into()),
            vec!["Recording part 1.mkv".into()],
            false,
        );
        notices.push(
            NoticeLevel::Warning,
            "Sources were kept".into(),
            "The result could not be saved, so the original files are still available".into(),
            Some("webp-0002".into()),
            vec!["Recording part 2.mkv".into()],
            false,
        );
        let now = Instant::now();
        for notice in &mut notices.items {
            notice.opened_at = now - NOTICE_FADE_IN;
            notice.expires_at = now + Duration::from_secs(30);
        }
        notices.items[1].expanded = true;
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, notices: &mut Notices| {
                crate::theme::install(ui.ctx());
                crate::theme::apply_preference(
                    ui.ctx(),
                    mediaa_core::settings::ThemePreference::Dark,
                );
                ui.painter().rect_filled(
                    ui.ctx().content_rect(),
                    0.0,
                    crate::theme::palette(ui).bg,
                );
                notices.show(ui.ctx());
            },
            notices,
        );
        harness.run_steps(3);
        harness.snapshot("notices");
    }

    #[test]
    fn closing_notice_is_removed_without_an_empty_frame() {
        let _ui = crate::app::test_support::ui_test_guard();
        let (mut notices, _) = notice();
        notices.items[0].closing_at = Some(Instant::now());
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, notices: &mut Notices| {
                notices.show(ui.ctx());
            },
            notices,
        );
        harness.run_steps(2);
        assert!(harness.state().items.is_empty());
    }

    #[test]
    fn removing_the_top_notice_does_not_shift_the_bottom_one() {
        let _ui = crate::app::test_support::ui_test_guard();
        let mut notices = Notices::default();
        for summary in ["Top", "Bottom"] {
            notices.push(
                NoticeLevel::Info,
                summary.into(),
                "Detail".into(),
                None,
                Vec::new(),
                false,
            );
        }
        let now = Instant::now();
        for notice in &mut notices.items {
            notice.opened_at = now - NOTICE_FADE_IN;
            notice.expires_at = now + Duration::from_secs(30);
        }
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, notices: &mut Notices| {
                notices.show(ui.ctx());
            },
            notices,
        );
        harness.run_steps(3);
        let before: Vec<_> = harness
            .get_all_by_label(icon::X)
            .map(|node| node.rect().bottom())
            .collect();
        assert_eq!(before.len(), 2);
        harness.get_all_by_label(icon::X).next().unwrap().click();
        harness.run_steps(3);
        let after: Vec<_> = harness
            .get_all_by_label(icon::X)
            .map(|node| node.rect().bottom())
            .collect();
        assert_eq!(after.len(), 1);
        assert!(
            (after[0] - before[1]).abs() < 0.5,
            "the remaining notice stays anchored: {before:?} -> {after:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn ui_regression_snapshot_notices_layers() {
        let _ui = crate::app::test_support::ui_test_guard();
        let mut notices = Notices::default();
        notices.push(
            NoticeLevel::Info,
            "Saved".into(),
            "The screenshot is ready".into(),
            None,
            Vec::new(),
            false,
        );
        let now = Instant::now();
        notices.items[0].opened_at = now - NOTICE_FADE_IN;
        notices.items[0].expires_at = now + Duration::from_secs(30);
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, notices: &mut Notices| {
                crate::theme::install(ui.ctx());
                crate::theme::apply_preference(
                    ui.ctx(),
                    mediaa_core::settings::ThemePreference::Dark,
                );
                ui.painter().rect_filled(
                    ui.ctx().content_rect(),
                    0.0,
                    crate::theme::palette(ui).bg,
                );
                egui::Area::new(egui::Id::new("fake-stop-overlay"))
                    .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
                    .order(egui::Order::Foreground)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::new()
                            .fill(crate::theme::palette(ui).card)
                            .inner_margin(egui::Margin::same(14))
                            .show(ui, |ui| {
                                ui.set_width(300.0);
                                ui.set_min_height(220.0);
                                ui.label("Recording controls");
                            });
                    });
                notices.show(ui.ctx());
                egui::Modal::new(egui::Id::new("notice-layer-modal")).show(ui.ctx(), |ui| {
                    ui.set_width(320.0);
                    ui.label("Confirm action");
                });
            },
            notices,
        );
        harness.run_steps(3);
        harness.snapshot("notices-layers");
    }
}
