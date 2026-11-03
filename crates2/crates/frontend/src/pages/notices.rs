use super::{notice_color, ActionNotice, Ctx};
use crate::ui::theme;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa2_core::notices::NoticeLevel;
use mediaa2_protocol::state::Notice;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

const NOTICE_WIDTH: f32 = 460.0;
const NOTICE_MAX: usize = 5;
const NOTICE_FADE_IN: Duration = Duration::from_millis(180);
const NOTICE_ANIMATION_FRAME: Duration = Duration::from_millis(16);
const NOTICE_DETAIL_HOLD: Duration = Duration::from_secs(8);

#[derive(Clone)]
struct NoticeItem {
    id: u64,
    backend_id: Option<u64>,
    level: NoticeLevel,
    summary: String,
    detail: String,
    opened_at: Instant,
    expires_at: Instant,
    hovered: bool,
    paused_remaining: Option<Duration>,
    expanded: bool,
    expired: bool,
}

#[derive(Clone, Default)]
struct NoticesState {
    items: Vec<NoticeItem>,
    known_backend: BTreeSet<u64>,
    hidden_backend: BTreeSet<u64>,
    next_id: u64,
    visible: bool,
}

impl NoticesState {
    fn sync(&mut self, backend: &[Notice], actions: Vec<ActionNotice>, now: Instant) {
        let live: BTreeSet<u64> = backend.iter().map(|notice| notice.id).collect();
        self.known_backend.retain(|id| live.contains(id));
        self.hidden_backend.retain(|id| live.contains(id));
        for notice in backend {
            if !self.known_backend.insert(notice.id) {
                continue;
            }
            if self.hidden_backend.contains(&notice.id) {
                continue;
            }
            self.push(
                Some(notice.id),
                notice.level,
                notice.summary.clone(),
                notice.detail.clone(),
                now,
            );
        }
        for notice in actions {
            self.push(None, notice.level, notice.summary, notice.detail, now);
        }
    }

    fn push(
        &mut self,
        backend_id: Option<u64>,
        level: NoticeLevel,
        summary: String,
        detail: String,
        now: Instant,
    ) {
        self.next_id += 1;
        self.items.push(NoticeItem {
            id: self.next_id,
            backend_id,
            level,
            summary,
            detail,
            opened_at: now,
            expires_at: now + lifetime(level),
            hovered: false,
            paused_remaining: None,
            expanded: false,
            expired: false,
        });
        while self.items.len() > NOTICE_MAX {
            self.items.remove(0);
        }
    }

    fn expire(&mut self, now: Instant) -> bool {
        let before = self.items.len();
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
                notice.expired = true;
            }
        }
        self.items.retain(|notice| !notice.expired);
        self.items.len() != before
    }

    fn close(&mut self, id: u64) {
        if let Some(notice) = self.items.iter().find(|notice| notice.id == id) {
            if let Some(backend_id) = notice.backend_id {
                self.hidden_backend.insert(backend_id);
            }
        }
        self.items.retain(|notice| notice.id != id);
    }

    fn remove_expired(&mut self) {
        self.items.retain(|notice| !notice.expired);
    }
}

fn lifetime(level: NoticeLevel) -> Duration {
    match level {
        NoticeLevel::Info | NoticeLevel::Success => Duration::from_secs(5),
        NoticeLevel::Warning => Duration::from_secs(8),
        NoticeLevel::Error => Duration::from_secs(10),
    }
}

fn level_icon(level: NoticeLevel) -> &'static str {
    match level {
        NoticeLevel::Info => icon::INFO,
        NoticeLevel::Success => icon::CHECK_CIRCLE,
        NoticeLevel::Warning => icon::WARNING,
        NoticeLevel::Error => icon::WARNING_OCTAGON,
    }
}

fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn notice_opacity(notice: &NoticeItem, now: Instant) -> f32 {
    let elapsed = now
        .saturating_duration_since(notice.opened_at)
        .as_secs_f32();
    smoothstep((elapsed / NOTICE_FADE_IN.as_secs_f32()).clamp(0.0, 1.0))
}

fn state_id() -> egui::Id {
    egui::Id::new("mediaa2-notices-state")
}

fn load_state(ctx: &egui::Context) -> NoticesState {
    ctx.data(|data| data.get_temp::<NoticesState>(state_id()))
        .unwrap_or_default()
}

fn store_state(ctx: &egui::Context, state: NoticesState) {
    ctx.data_mut(|data| data.insert_temp(state_id(), state));
}

pub fn show(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let now = Instant::now();
    let mut state = load_state(ui.ctx());
    let actions = ctx.actions.take_notices();
    state.sync(ctx.view.notices(), actions, now);
    if state.expire(now) {
        ui.ctx().request_repaint();
    }
    if state.items.is_empty() {
        state.visible = false;
        store_state(ui.ctx(), state);
        return;
    }
    let first_show = !state.visible;
    state.visible = true;
    let mut close = None;
    egui::Area::new(egui::Id::new("mediaa2-notices"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
        .order(egui::Order::Foreground)
        .sizing_pass(first_show)
        .show(ui.ctx(), |ui| {
            let p = theme::palette(ui);
            ui.set_max_width(NOTICE_WIDTH);
            ui.spacing_mut().item_spacing.y = 6.0;
            for notice in &mut state.items {
                let opacity = notice_opacity(notice, now);
                let color = notice_color(notice.level, ui);
                let notice_icon = level_icon(notice.level);
                let expandable = !notice.detail.is_empty();
                let mut card_close = false;
                let mut card_toggle = false;
                let response = ui
                    .scope(|ui| {
                        ui.set_opacity(opacity);
                        egui::Frame::new()
                            .fill(p.card)
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(egui::CornerRadius::same(4))
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.set_width(NOTICE_WIDTH - 20.0);
                                ui.horizontal(|ui| {
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(20.0, 22.0),
                                        egui::Layout::centered_and_justified(
                                            egui::Direction::LeftToRight,
                                        ),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new(notice_icon)
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
                                                    egui::RichText::new(&notice.summary).size(13.0),
                                                )
                                                .wrap()
                                                .sense(egui::Sense::click()),
                                            );
                                            if summary.clicked() && expandable {
                                                card_toggle = true;
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
                                            if expandable
                                                && ui
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
                                                card_toggle = true;
                                            }
                                        },
                                    );
                                });
                                if notice.expanded && expandable {
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
                                }
                            });
                    })
                    .response;
                notice.hovered = response.contains_pointer();
                if card_close {
                    close = Some(notice.id);
                } else if card_toggle {
                    notice.expanded = !notice.expanded;
                    if notice.expanded {
                        notice.expires_at =
                            notice.expires_at.max(Instant::now() + NOTICE_DETAIL_HOLD);
                    }
                }
            }
        });
    if let Some(id) = close {
        state.close(id);
        ui.ctx().request_repaint();
    }
    state.remove_expired();
    let mut wake = None;
    for notice in &state.items {
        if now.saturating_duration_since(notice.opened_at) < NOTICE_FADE_IN {
            wake = Some(NOTICE_ANIMATION_FRAME);
            break;
        }
        if !notice.hovered {
            let until_expiry = notice.expires_at.saturating_duration_since(now);
            wake = Some(wake.map_or(until_expiry, |current: Duration| current.min(until_expiry)));
        }
    }
    if let Some(wake) = wake {
        ui.ctx().request_repaint_after(wake);
    }
    store_state(ui.ctx(), state);
}

#[cfg(test)]
pub(crate) fn age_notices(ctx: &egui::Context, age: Duration, lifetime: Duration) {
    let now = Instant::now();
    let mut state = load_state(ctx);
    for notice in &mut state.items {
        notice.opened_at = now - age;
        notice.expires_at = now + lifetime;
        notice.hovered = false;
        notice.paused_remaining = None;
    }
    store_state(ctx, state);
}

#[cfg(test)]
pub(crate) fn expand_notice(ctx: &egui::Context, index: usize) {
    let mut state = load_state(ctx);
    if let Some(notice) = state.items.get_mut(index) {
        notice.expanded = true;
    }
    store_state(ctx, state);
}

#[cfg(test)]
pub(crate) fn expire_notice(ctx: &egui::Context, index: usize) {
    let mut state = load_state(ctx);
    if let Some(notice) = state.items.get_mut(index) {
        notice.expires_at = Instant::now();
        notice.paused_remaining = None;
    }
    store_state(ctx, state);
}

#[cfg(test)]
pub(crate) fn notice_hovered(ctx: &egui::Context, index: usize) -> bool {
    load_state(ctx)
        .items
        .get(index)
        .is_some_and(|notice| notice.hovered)
}

#[cfg(test)]
pub(crate) fn notice_count(ctx: &egui::Context) -> usize {
    load_state(ctx).items.len()
}
