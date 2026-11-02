use crate::features::notices::{
    AppNotice, NoticeAction, NoticeLevel, Notices, NOTICE_ANIMATION_FRAME, NOTICE_FADE_IN,
    NOTICE_WIDTH,
};
use crate::ui::theme;
use eframe::egui;
use egui_phosphor::regular as icon;
use rust_i18n::t;
use std::time::{Duration, Instant};
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

pub(crate) trait NoticesUi {
    fn show(&mut self, ctx: &egui::Context) -> Option<NoticeAction>;
    fn show_blocked(&mut self, ctx: &egui::Context) -> Option<NoticeAction>;
    fn show_with(&mut self, ctx: &egui::Context, interactive: bool) -> Option<NoticeAction>;
}

impl NoticesUi for Notices {
    fn show(&mut self, ctx: &egui::Context) -> Option<NoticeAction> {
        self.show_with(ctx, true)
    }

    fn show_blocked(&mut self, ctx: &egui::Context) -> Option<NoticeAction> {
        self.show_with(ctx, false)
    }

    fn show_with(&mut self, ctx: &egui::Context, interactive: bool) -> Option<NoticeAction> {
        if !self.enabled {
            return None;
        }
        let before = self.items.len();
        self.items.retain(|notice| notice.closing_at.is_none());
        let removed_any = self.items.len() != before;
        if removed_any {
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
                                                                    icon::CARET_DOWN
                                                                } else {
                                                                    icon::CARET_UP
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
        let before_remove = self.items.len();
        self.items.retain(|notice| !remove.contains(&notice.id));
        if self.items.len() != before_remove {
            ctx.request_repaint();
        }
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
        let _ui = crate::test_support::ui_test_guard();
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
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
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
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
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
                crate::ui::theme::install(ui.ctx());
                crate::ui::theme::apply_preference(
                    ui.ctx(),
                    mediaa_core::settings::ThemePreference::Dark,
                );
                ui.painter().rect_filled(
                    ui.ctx().content_rect(),
                    0.0,
                    crate::ui::theme::palette(ui).bg,
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
        let _ui = crate::test_support::ui_test_guard();
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
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
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

    #[test]
    fn an_expired_notice_is_dropped_with_an_immediate_repaint() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
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
        let bottom_before = harness
            .get_all_by_label(icon::X)
            .last()
            .expect("the newest notice")
            .rect();
        harness.state_mut().items[0].expires_at = Instant::now();
        harness.run_steps(1);
        assert_eq!(
            harness.state().items.len(),
            1,
            "the expired notice is dropped"
        );
        harness.run_steps(1);
        assert!(
            harness.ctx.requested_repaint_last_pass(),
            "the stack has to be laid out again at once, otherwise the remaining card keeps the shifted position"
        );
        harness.run_steps(1);
        let bottom_after = harness
            .get_all_by_label(icon::X)
            .last()
            .expect("the remaining notice")
            .rect();
        assert!(
            (bottom_after.bottom() - bottom_before.bottom()).abs() < 0.5,
            "the remaining notice keeps its place: {bottom_before:?} -> {bottom_after:?}"
        );
    }

    #[cfg(windows)]
    fn notice_stack() -> egui_kittest::Harness<'static, Notices> {
        let mut notices = Notices::default();
        notices.push(
            NoticeLevel::Success,
            "Screenshot copied and saved".into(),
            "Screenshot_2026-11-01_10-13-26.png".into(),
            None,
            Vec::new(),
            false,
        );
        notices.push(
            NoticeLevel::Info,
            "Files added to the conversion queue: 2".into(),
            "Screenshot_2026-11-01_10-27-36.png\nScreenshot_2026-11-01_10-31-02.png".into(),
            None,
            Vec::new(),
            false,
        );
        let now = Instant::now();
        for notice in &mut notices.items {
            notice.opened_at = now - NOTICE_FADE_IN;
            notice.expires_at = now + Duration::from_secs(30);
        }
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, notices: &mut Notices| {
                crate::ui::theme::install(ui.ctx());
                crate::ui::theme::apply_preference(
                    ui.ctx(),
                    mediaa_core::settings::ThemePreference::Dark,
                );
                ui.painter().rect_filled(
                    ui.ctx().content_rect(),
                    0.0,
                    crate::ui::theme::palette(ui).bg,
                );
                notices.show(ui.ctx());
            },
            notices,
        );
        harness.run_steps(3);
        harness
    }

    #[cfg(windows)]
    fn bottom_notice_bottom(harness: &egui_kittest::Harness<'_, Notices>) -> f32 {
        harness
            .get_all_by_label(icon::X)
            .last()
            .expect("the newest notice")
            .rect()
            .bottom()
    }

    #[cfg(windows)]
    #[test]
    fn ui_regression_snapshot_notices_stack() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let mut harness = notice_stack();
        harness.run_steps(2);
        harness.snapshot("notices-stack-two");
    }

    #[cfg(windows)]
    #[test]
    fn ui_regression_snapshot_notices_stack_after_close() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let mut harness = notice_stack();
        let before = bottom_notice_bottom(&harness);
        harness
            .get_all_by_label(icon::X)
            .next()
            .expect("the top notice")
            .click();
        harness.run_steps(5);
        assert_eq!(harness.state().items.len(), 1, "the top notice is gone");
        let after = bottom_notice_bottom(&harness);
        assert!(
            (after - before).abs() < 0.5,
            "the remaining notice keeps its place: {before} -> {after}"
        );
        harness.hover_at(egui::pos2(20.0, 20.0));
        harness.run_steps(2);
        harness.snapshot("notices-stack-after-close");
    }

    #[cfg(windows)]
    #[test]
    fn ui_regression_snapshot_notices_stack_after_expiry() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let mut harness = notice_stack();
        let before = bottom_notice_bottom(&harness);
        harness.state_mut().items[0].expires_at = Instant::now();
        harness.run_steps(5);
        assert_eq!(harness.state().items.len(), 1, "the expired notice is gone");
        let after = bottom_notice_bottom(&harness);
        assert!(
            (after - before).abs() < 0.5,
            "the remaining notice keeps its place: {before} -> {after}"
        );
        harness.snapshot("notices-stack-after-expiry");
    }

    #[cfg(windows)]
    #[test]
    fn ui_regression_snapshot_notices_layers() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
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
                crate::ui::theme::install(ui.ctx());
                crate::ui::theme::apply_preference(
                    ui.ctx(),
                    mediaa_core::settings::ThemePreference::Dark,
                );
                ui.painter().rect_filled(
                    ui.ctx().content_rect(),
                    0.0,
                    crate::ui::theme::palette(ui).bg,
                );
                egui::Area::new(egui::Id::new("fake-stop-overlay"))
                    .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
                    .order(egui::Order::Foreground)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::new()
                            .fill(crate::ui::theme::palette(ui).card)
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

    #[test]
    fn notice_expands_to_the_full_context_and_keeps_its_id() {
        let _ui_guard = crate::test_support::ui_test_guard();
        let mut notices = Notices::default();
        let summary = "Скриншот 3200×2000 скопирован и сохранён: Recording screenshot.png";
        notices.push(
            NoticeLevel::Success,
            summary.into(),
            "Полный технический контекст без сокращений".into(),
            Some("webp-1234-0001".into()),
            vec![
                "C:\\Videos\\Recording source part 1.mkv".into(),
                "C:\\Videos\\Recording source part 2.mkv".into(),
            ],
            true,
        );
        let observed = std::sync::Arc::new(std::sync::Mutex::new(None));
        let sink = observed.clone();
        let mut harness = egui_kittest::Harness::new_ui_state(
            move |ui: &mut egui::Ui, notices: &mut Notices| {
                if let Some(action) = notices.show(ui.ctx()) {
                    *sink.lock().unwrap() = Some(action);
                }
            },
            notices,
        );
        crate::ui::theme::install(&harness.ctx);
        harness.run_steps(20);
        let log_label = format!("{} webp-1234-0001", rust_i18n::t!("notice.log_id"));
        let copy_label = rust_i18n::t!("notice.copy").to_string();
        assert!(harness.query_by_label(summary).is_some());
        assert!(harness.query_by_label(&log_label).is_none());
        assert!(harness
            .query_by_label("Полный технический контекст без сокращений")
            .is_none());
        assert!(
            harness.query_by_label(icon::CARET_UP).is_some(),
            "a collapsed notice expands upwards"
        );
        assert!(harness.query_by_label(icon::CARET_DOWN).is_none());
        if let Ok(path) = std::env::var("MEDIAA_NOTICE_SNAPSHOT") {
            std::thread::sleep(Duration::from_millis(200));
            harness.step();
            harness
                .render()
                .unwrap()
                .save(format!("{path}-collapsed.png"))
                .unwrap();
            let mut expanded = harness.state().clone();
            expanded.items[0].expanded = true;
            let mut preview = egui_kittest::Harness::new_ui_state(
                |ui: &mut egui::Ui, notices: &mut Notices| {
                    notices.show(ui.ctx());
                },
                expanded,
            );
            crate::ui::theme::install(&preview.ctx);
            preview.run_steps(20);
            preview
                .render()
                .unwrap()
                .save(format!("{path}-expanded.png"))
                .unwrap();
        }
        harness.get_by_label(icon::CARET_UP).click();
        harness.run_steps(20);
        assert!(harness
            .query_by_label("Полный технический контекст без сокращений")
            .is_some());
        assert!(harness.query_by_label(&log_label).is_some());
        assert!(harness.query_by_label(icon::CARET_DOWN).is_some());
        harness.get_by_label(&copy_label).click();
        harness.run_steps(20);
        assert_eq!(
            observed.lock().unwrap().clone(),
            Some(NoticeAction::CopyLog("webp-1234-0001".into()))
        );
        harness.get_by_label(icon::CARET_DOWN).click();
        harness.run_steps(20);
        assert!(harness
            .query_by_label("Полный технический контекст без сокращений")
            .is_none());
        assert_eq!(harness.state().items[0].id, 1);
    }

    #[test]
    fn expanded_notice_expires_without_leaving_an_empty_frame() {
        let _ui_guard = crate::test_support::ui_test_guard();
        let mut notices = Notices::default();
        notices.push(
            NoticeLevel::Info,
            "Transient".into(),
            "Expanded detail".into(),
            None,
            Vec::new(),
            false,
        );
        notices.items[0].expanded = true;
        notices.items[0].expires_at = Instant::now();
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui: &mut egui::Ui, notices: &mut Notices| {
                notices.show(ui.ctx());
            },
            notices,
        );
        crate::ui::theme::install(&harness.ctx);
        harness.step();
        assert!(harness.state().items.is_empty());
    }

    #[test]
    fn notice_icon_and_title_are_vertically_centred() {
        let _ui_guard = crate::test_support::ui_test_guard();
        let mut notices = Notices::default();
        notices.push(
            NoticeLevel::Success,
            "Copied".into(),
            "Detail".into(),
            None,
            Vec::new(),
            false,
        );
        std::thread::sleep(Duration::from_millis(200));
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui: &mut egui::Ui, notices: &mut Notices| {
                notices.show(ui.ctx());
            },
            notices,
        );
        crate::ui::theme::install(&harness.ctx);
        harness.run_steps(20);
        let icon = harness.get_by_label(icon::CHECK_CIRCLE).rect();
        let title = harness.get_by_label("Copied").rect();
        assert!(
            (icon.center().y - title.center().y).abs() <= 1.0,
            "icon={icon:?} title={title:?}"
        );
    }
}
