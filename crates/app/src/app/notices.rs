use super::{recovery_links, App};
use crate::theme;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa_media::error::MediaErrorKind;
use mediaa_platform::{clipboard, files};
use rust_i18n::t;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const NOTICE_SUMMARY_CHARS: usize = 72;

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
    pub(crate) expires_at: Option<Instant>,
}

#[derive(Default, Clone)]
pub(crate) struct Notices {
    pub(crate) items: Vec<AppNotice>,
    next_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoticeAction {
    CopyLog(String),
    CopyFiles(Vec<PathBuf>),
    CopyFile(PathBuf),
    RevealFile(PathBuf),
}

impl Notices {
    pub(crate) fn push(
        &mut self,
        level: NoticeLevel,
        summary: String,
        detail: String,
        log_ref: Option<String>,
        files: Vec<PathBuf>,
        persistent: bool,
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
        let lifetime = match level {
            NoticeLevel::Info | NoticeLevel::Success => Duration::from_secs(5),
            NoticeLevel::Warning => Duration::from_secs(8),
            NoticeLevel::Error => Duration::from_secs(10),
        };
        self.items.push(AppNotice {
            id,
            level,
            summary,
            detail,
            log_ref,
            files,
            expanded: false,
            expires_at: (!persistent).then(|| Instant::now() + lifetime),
        });
        while self.items.len() > 5 {
            let index = self
                .items
                .iter()
                .position(|notice| notice.expires_at.is_some())
                .unwrap_or(0);
            self.items.remove(index);
        }
    }

    fn remove(&mut self, id: u64) {
        self.items.retain(|notice| notice.id != id);
    }

    fn expire(&mut self) {
        let now = Instant::now();
        self.items.retain(|notice| {
            notice.expanded || notice.expires_at.is_none_or(|expires| expires > now)
        });
    }

    pub(crate) fn show(&mut self, ctx: &egui::Context) -> Option<NoticeAction> {
        let mut close = None;
        let mut copy_log = None;
        let mut copy_files = None;
        let mut copy_file = None;
        let mut reveal_file = None;
        self.expire();
        egui::Area::new(egui::Id::new("app-notices"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let p = theme::palette(ui);
                ui.set_max_width(460.0);
                ui.spacing_mut().item_spacing.y = 6.0;
                for notice in &mut self.items {
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
                    let estimated = notice.summary.chars().count() as f32 * 6.6 + 88.0;
                    let width = if notice.expanded {
                        460.0
                    } else {
                        estimated.clamp(280.0, 480.0)
                    };
                    let mut toggle = false;
                    let mut card_close = false;
                    let mut card_copy_log = false;
                    let mut card_copy_files = false;
                    let mut card_copy_file = None;
                    let mut card_reveal_file = None;
                    let response = egui::Frame::new()
                        .fill(p.card)
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(egui::CornerRadius::same(4))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(width - 20.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(icon).color(color).size(16.0));
                                let text_width = (ui.available_width() - 54.0).max(120.0);
                                ui.allocate_ui_with_layout(
                                    egui::vec2(text_width, 0.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        let summary = ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&notice.summary).size(13.0),
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
                                                    egui::RichText::new(if notice.expanded {
                                                        "▴"
                                                    } else {
                                                        "▾"
                                                    })
                                                    .color(p.text_weak)
                                                    .size(13.0),
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
                                                egui::RichText::new(&notice.detail).size(12.0),
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
                                            for (index, file) in notice.files.iter().enumerate() {
                                                let name = file
                                                    .file_name()
                                                    .unwrap_or_default()
                                                    .to_string_lossy();
                                                ui.push_id(
                                                    ("notice-file", notice.id, index),
                                                    |ui| {
                                                        ui.horizontal(|ui| {
                                                            let actions_width = 140.0;
                                                            let text_width = (ui.available_width()
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
                                                                card_copy_file = Some(file.clone());
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
                        });
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
                        notice.expires_at = if notice.expanded {
                            None
                        } else {
                            Some(Instant::now() + Duration::from_secs(8))
                        };
                    } else if response.response.hovered() {
                        if let Some(expires) = notice.expires_at.as_mut() {
                            *expires = Instant::now() + Duration::from_secs(3);
                        }
                    }
                }
            });
        if let Some(id) = close {
            self.remove(id);
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
        let detail = msg.into().trim().to_string();
        let summary = notice_summary(&detail, &t!("notice.error"));
        self.notices.push(
            NoticeLevel::Error,
            summary,
            detail,
            None,
            Vec::new(),
            kind == MediaErrorKind::Io,
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

    pub(super) fn show_notices(&mut self, ctx: &egui::Context) {
        match self.notices.show(ctx) {
            Some(NoticeAction::CopyLog(log_ref)) => match clipboard::copy_text(&log_ref) {
                Ok(()) => self.success(t!("notice.log_id_copied", id = log_ref)),
                Err(error) => self.error(t!("err.copy_failed", error = format!("{error:#}"))),
            },
            Some(NoticeAction::CopyFiles(files)) => {
                let links = recovery_links(&files);
                match clipboard::copy_text(&links) {
                    Ok(()) => self.success(t!("toast.recovery_copied")),
                    Err(error) => self.error(t!("err.copy_failed", error = format!("{error:#}"))),
                }
            }
            Some(NoticeAction::CopyFile(file)) => {
                match clipboard::copy_text(&mediaa_core::library::file_url(&file)) {
                    Ok(()) => self.success(t!("toast.recovery_copied")),
                    Err(error) => self.error(t!("err.copy_failed", error = format!("{error:#}"))),
                }
            }
            Some(NoticeAction::RevealFile(file)) => {
                if let Err(error) = files::reveal(&file) {
                    self.error(format!("{error:#}"));
                }
            }
            None => {}
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
