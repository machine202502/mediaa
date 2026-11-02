pub mod about;
pub mod agent;
pub mod audio;
pub mod capture;
pub mod convert;
pub(crate) mod ctx;
pub mod library;
pub mod record;
pub mod settings;

use crate::features::command::{AppCommand, RecordingCommand};
use crate::features::navigation::Page;
use crate::features::notices::NoticeAction;
use crate::features::region_editor;
use crate::ui::notices::NoticesUi;
use crate::ui::pages::ctx::Ctx;
use crate::{i18n, ui::theme};
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};
use egui_phosphor::regular as icon;
use mediaa_core::library::human_duration;
use mediaa_core::settings::{RecordPreset, RecordProfile, ThemePreference};
use rust_i18n::t;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Library,
    Capture,
    Convert,
    Agent,
}

pub(crate) fn preset_combo(
    ui: &mut egui::Ui,
    id: egui::Id,
    current: Option<RecordPreset>,
    presets: &[RecordPreset],
) -> Option<RecordPreset> {
    let label = |preset: RecordPreset| match preset {
        RecordPreset::Quality => t!("record.preset_quality"),
        RecordPreset::Medium => t!("record.preset_medium"),
        RecordPreset::Fast => t!("record.preset_fast"),
    };
    let selected = current
        .map(label)
        .unwrap_or_else(|| t!("record.preset_custom"));
    let mut picked = None;
    crate::ui::theme::combo_box(id)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for preset in presets {
                if ui
                    .selectable_label(current == Some(*preset), label(*preset))
                    .clicked()
                {
                    picked = Some(*preset);
                }
            }
        });
    picked
}

pub(crate) trait PageUi {
    fn section(self) -> Option<Section>;
}

impl PageUi for Page {
    fn section(self) -> Option<Section> {
        match self {
            Page::Library => Some(Section::Library),
            Page::Screenshot | Page::Record | Page::RecordAudio => Some(Section::Capture),
            Page::Convert => Some(Section::Convert),
            Page::Catalogs | Page::Providers | Page::Models | Page::Queue | Page::Budget => {
                Some(Section::Agent)
            }
            Page::Settings | Page::About => None,
        }
    }
}

fn sub_pages(section: Section) -> Vec<(Page, &'static str, String)> {
    match section {
        Section::Capture => vec![
            (Page::Screenshot, icon::CAMERA, t!("nav.screenshot").into()),
            (Page::Record, icon::VIDEO_CAMERA, t!("nav.record").into()),
            (
                Page::RecordAudio,
                icon::MICROPHONE,
                t!("nav.record_audio").into(),
            ),
        ],
        Section::Agent => vec![
            (
                Page::Catalogs,
                icon::TREE_STRUCTURE,
                t!("nav.catalogs").into(),
            ),
            (Page::Queue, icon::QUEUE, t!("nav.queue").into()),
            (Page::Budget, icon::GAUGE, t!("nav.budget").into()),
            (
                Page::Providers,
                icon::PLUGS_CONNECTED,
                t!("nav.providers").into(),
            ),
            (Page::Models, icon::CUBE, t!("nav.models").into()),
        ],
        Section::Library | Section::Convert => Vec::new(),
    }
}

pub fn frame(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) -> Option<NoticeAction> {
    chrome(ctx, ui);
    let gui = ui.ctx().clone();
    let interactive = gui.memory(|memory| memory.allows_interaction(ui.layer_id()));
    ctx.notices.set_enabled(ctx.settings.notifications_enabled);
    let action = if ctx.viewer_open {
        None
    } else if interactive {
        ctx.notices.show(&gui)
    } else {
        ctx.notices.show_blocked(&gui)
    };
    page(ctx, ui);
    action
}

pub fn chrome(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    let gui = ui.ctx().clone();
    egui::Panel::top("header")
        .resizable(false)
        .exact_size(46.0)
        .frame(
            egui::Frame::new()
                .fill(p.chrome)
                .inner_margin(Margin::symmetric(12, 6))
                .stroke(Stroke::new(1.0, p.border)),
        )
        .show(ui, |ui| header_bar(ctx, ui, &gui));

    if let Some(section) = ctx.page.section() {
        ctx.last_main_page = ctx.page;
        match section {
            Section::Capture => ctx.last_capture_page = ctx.page,
            Section::Agent => ctx.last_agent_page = ctx.page,
            Section::Library | Section::Convert => {}
        }
        let pages = sub_pages(section);
        if !pages.is_empty() {
            let subnav = egui::Panel::top("subnav")
                .resizable(false)
                .exact_size(36.0)
                .frame(
                    egui::Frame::new()
                        .fill(p.chrome)
                        .inner_margin(Margin::symmetric(12, 4)),
                )
                .show(ui, |ui| sub_nav(ctx, ui, &pages));
            ui.painter().hline(
                subnav.response.rect.x_range(),
                subnav.response.rect.bottom() - 0.5,
                Stroke::new(1.0, p.border),
            );
        }
    }

    #[cfg(test)]
    let convert_dir = ctx.settings.convert_dir();
    if ctx.page == Page::Convert {
        egui::Panel::bottom("convert-start")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(18, 10)),
            )
            .show(ui, |ui| {
                let ready = ctx.conversion.can_start();
                ui.vertical_centered(|ui| {
                    let btn = theme::primary(ui, icon::SHUFFLE, &t!("convert.start"))
                        .min_size(egui::vec2(240.0, 38.0));
                    if ui.add_enabled(ready, btn).clicked() {
                        #[cfg(test)]
                        if let Some(error) = ctx.conversion.start_from_ui(ctx.wake, &convert_dir) {
                            ctx.error(error);
                        }
                        #[cfg(not(test))]
                        if let Some(start) = ctx.conversion.backend_start() {
                            ctx.intents
                                .push(crate::features::intent::Intent::ConversionStart(start));
                        }
                    }
                });
            });
    }

    if matches!(ctx.page, Page::Record | Page::RecordAudio) {
        egui::Panel::bottom("record-start")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(18, 10)),
            )
            .show(ui, |ui| {
                let live = !ctx.recording.is_idle();
                ui.add_enabled_ui(!live, |ui| {
                    if ctx.page == Page::Record {
                        let target = ctx.record_target_from_ui().map(|_| ());
                        if record::start_bar(ctx.settings, ctx.recovery_active(), &target, ui) {
                            ctx.start_recording_from_ui();
                        }
                    } else if audio::start_bar(
                        ctx.settings,
                        ctx.recovery_active(),
                        mediaa_platform::audio::system_audio_supported(),
                        ui,
                    ) {
                        ctx.start_audio_recording(RecordProfile::App, None);
                    }
                });
            });
    }

    if matches!(ctx.page, Page::Record | Page::RecordAudio) {
        let live = !ctx.recording.is_idle();
        let gui = ui.ctx().clone();
        let hidden_starting =
            ctx.settings.hide_recording_preparation && ctx.recording.is_starting();
        let show = live && !hidden_starting;
        let shown_id = egui::Id::new("record-status-shown");
        let was_shown = gui
            .data(|data| data.get_temp::<bool>(shown_id))
            .unwrap_or(false);
        gui.data_mut(|data| data.insert_temp(shown_id, show));
        if show {
            let width = 640.0_f32.min((gui.content_rect().width() - 36.0).max(280.0));
            let overlay_bottom = gui.content_rect().bottom() - 56.0 - 18.0;
            egui::Area::new(egui::Id::new("record-status-overlay"))
                .order(egui::Order::Foreground)
                .fade_in(false)
                .constrain(false)
                .sizing_pass(!was_shown)
                .pivot(egui::Align2::CENTER_BOTTOM)
                .default_size(egui::vec2(width, 166.0))
                .fixed_pos(egui::pos2(gui.content_rect().center().x, overlay_bottom))
                .show(&gui, |ui| {
                    ui.set_width(width);
                    if let Some(command) = record::status::live_status(
                        ctx.recording,
                        ui,
                        &gui,
                        ctx.settings.hide_recording_preparation,
                    ) {
                        let command = match command {
                            record::status::RecordingUiCommand::TogglePause => {
                                RecordingCommand::TogglePause
                            }
                            record::status::RecordingUiCommand::Stop(action) => {
                                RecordingCommand::Stop {
                                    action,
                                    source: crate::features::recording::StopSource::Ui,
                                }
                            }
                        };
                        ctx.dispatch_command(AppCommand::Recording(command));
                    }
                });
        }
    }
    if ctx.page == Page::Convert {
        if let Some(status) = ctx.conversion.status() {
            let gui = ui.ctx().clone();
            let width = 640.0_f32.min((gui.content_rect().width() - 36.0).max(280.0));
            let bottom = gui.content_rect().bottom() - 56.0 - 18.0;
            let time = human_duration(status.elapsed.as_secs_f64());
            let detail = (status.total > 1).then(|| {
                t!(
                    "convert.batch_progress",
                    done = status.done + 1,
                    total = status.total
                )
                .to_string()
            });
            let cancel_label = t!("common.cancel").into_owned();
            let mut cancel = false;
            egui::Area::new(egui::Id::new("convert-status-overlay"))
                .order(egui::Order::Foreground)
                .fade_in(false)
                .constrain(false)
                .pivot(egui::Align2::CENTER_BOTTOM)
                .fixed_pos(egui::pos2(gui.content_rect().center().x, bottom))
                .show(&gui, |ui| {
                    ui.set_width(width);
                    cancel = theme::status_card(
                        ui,
                        theme::StatusCard {
                            icon: icon::SHUFFLE,
                            color: theme::palette(ui).accent,
                            time: &time,
                            label: &t!("convert.status_running"),
                            detail: detail.as_deref(),
                            progress: status.progress,
                            action: Some((icon::X, &cancel_label)),
                        },
                    );
                });
            gui.request_repaint_after(Duration::from_millis(250));
            if cancel {
                ctx.conversion.cancel();
            }
        }
    }
}

pub fn page(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    if ctx.page == Page::Catalogs {
        agent::catalogs::screen(ctx, ui);
        return;
    }

    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(p.bg)
                .inner_margin(Margin::symmetric(18, 14)),
        )
        .show(ui, |ui| {
            let offset = ctx
                .page_scroll
                .iter()
                .find(|(page, _)| *page == ctx.page)
                .map_or(0.0, |(_, offset)| *offset);
            let scroll = egui::ScrollArea::vertical()
                .id_salt(format!("page-scroll-{:?}", ctx.page))
                .auto_shrink([false, false])
                .vertical_scroll_offset(offset)
                .show(ui, |ui| match ctx.page {
                    Page::Screenshot => {
                        let intents = capture::ui(ctx.settings, ctx.monitors, ctx.windows, ui);
                        ctx.handle_intents(intents);
                    }
                    Page::Record => {
                        let actions = record::ui(
                            ctx.recording,
                            ctx.settings,
                            ctx.monitors,
                            ctx.windows,
                            ctx.mic_devices,
                            ui,
                        );
                        for action in actions {
                            match action {
                                record::RecordUiAction::SettingsChanged => ctx.settings_changed(),
                                record::RecordUiAction::RefreshCatalogs => ctx.refresh_catalogs(),
                                record::RecordUiAction::SelectRegion => ctx
                                    .start_region_screenshot(region_editor::Purpose::RecordRegion),
                            }
                        }
                    }
                    Page::RecordAudio => {
                        let view = audio::AudioView {
                            live: !ctx.recording.is_idle(),
                            system_audio: mediaa_platform::audio::system_audio_supported(),
                        };
                        let intents = audio::ui(view, ctx.settings, ctx.mic_devices, ui);
                        ctx.handle_intents(intents);
                    }
                    Page::Library => {
                        library::dialogs(ctx, ui.ctx());
                        let actions = ctx.lib.ui(
                            ctx.settings,
                            ctx.wake,
                            ctx.thumbs,
                            ctx.previews,
                            &mut ctx.caches.viewer,
                            ui,
                        );
                        for action in actions {
                            library::apply_action(ctx, ui.ctx(), action);
                        }
                    }
                    Page::Convert => {
                        let convert_dir = ctx.settings.convert_dir();
                        let before = ctx.settings.convert_dir.clone();
                        if let Some(action) = ctx.conversion.ui(
                            ui,
                            ctx.wake,
                            &convert_dir,
                            &mut ctx.settings.convert_dir,
                        ) {
                            ctx.apply_conversion_ui_action(action);
                        }
                        if ctx.settings.convert_dir != before {
                            ctx.settings_changed();
                        }
                    }
                    Page::Providers => agent::providers::ui(ctx, ui),
                    Page::Models => agent::models::ui(ctx, ui),
                    Page::Queue => agent::queue::ui(ctx, ui),
                    Page::Budget => agent::budget::ui(ctx, ui),
                    Page::Catalogs => {}
                    Page::Settings => settings::ui(ctx, ui),
                    Page::About => {
                        let intents = about::ui(ctx.settings.theme, ctx.caches, ui);
                        ctx.handle_intents(intents);
                    }
                });
            if let Some(entry) = ctx
                .page_scroll
                .iter_mut()
                .find(|(page, _)| *page == ctx.page)
            {
                entry.1 = scroll.state.offset.y;
            } else {
                ctx.page_scroll.push((ctx.page, scroll.state.offset.y));
            }
        });
}

fn header_bar(ctx: &mut Ctx<'_>, ui: &mut egui::Ui, gui: &egui::Context) {
    let p = theme::palette(ui);
    ui.horizontal_centered(|ui| {
        let logo = ctx.caches.logo(gui, ctx.settings.theme);
        ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(24.0, 24.0)));
        ui.label(RichText::new("MediaA").size(15.0).strong());
        ui.add_space(14.0);

        let mut section = ctx.page.section();
        let before = section;
        let nav = [
            (
                Some(Section::Library),
                format!("{}  {}", icon::IMAGES, t!("section.library")),
            ),
            (
                Some(Section::Capture),
                format!("{}  {}", icon::SCREENCAST, t!("section.capture")),
            ),
            (
                Some(Section::Convert),
                format!("{}  {}", icon::SHUFFLE, t!("section.convert")),
            ),
            (
                Some(Section::Agent),
                format!("{}  {}", icon::SPARKLE, t!("section.agent")),
            ),
        ];
        let mut marks: Vec<(Option<Section>, Color32)> = Vec::new();
        if !ctx.recording.is_idle() {
            marks.push((Some(Section::Capture), p.danger));
        }
        if ctx.conversion.is_busy() {
            marks.push((Some(Section::Convert), p.success));
        }
        if gui.content_rect().width() < 820.0 {
            let selected = nav
                .iter()
                .find(|(option, _)| *option == section)
                .map(|(_, label)| label.clone())
                .unwrap_or_default();
            let font = ui
                .style()
                .text_styles
                .get(&egui::TextStyle::Button)
                .cloned()
                .unwrap_or_else(|| egui::FontId::proportional(14.0));
            let text_width = nav
                .iter()
                .map(|(_, label)| {
                    ui.fonts_mut(|fonts| {
                        fonts
                            .layout_no_wrap(label.clone(), font.clone(), egui::Color32::WHITE)
                            .size()
                            .x
                    })
                })
                .fold(0.0_f32, f32::max);
            let selected_mark = marks
                .iter()
                .find(|(option, _)| *option == section)
                .map(|(_, color)| *color);
            let selected_text = if selected_mark.is_some() {
                format!("\u{00A0}\u{00A0}\u{00A0}\u{00A0}{selected}")
            } else {
                selected.clone()
            };
            let combo = crate::ui::theme::combo_box("header-section")
                .width(text_width + 34.0 + if selected_mark.is_some() { 22.0 } else { 0.0 })
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    for (option, label) in &nav {
                        ui.horizontal(|ui| {
                            if let Some(color) = marks
                                .iter()
                                .find(|(marked, _)| marked == option)
                                .map(|(_, color)| *color)
                            {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(9.0, 9.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().circle_filled(rect.center(), 3.0, color);
                            }
                            ui.selectable_value(&mut section, *option, label);
                        });
                    }
                });
            if let Some(color) = selected_mark {
                ui.painter().circle_filled(
                    combo.response.rect.left_center() + egui::vec2(9.0, 0.0),
                    3.0,
                    color,
                );
            }
        } else {
            theme::segmented_marked_many(ui, &mut section, &nav, &marks);
        }
        if section != before {
            ctx.page = match section {
                Some(Section::Library) => Page::Library,
                Some(Section::Capture) | None => ctx.last_capture_page,
                Some(Section::Convert) => Page::Convert,
                Some(Section::Agent) => ctx.last_agent_page,
            };
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (page, ic, tip) in [
                (Page::Settings, icon::GEAR, t!("nav.settings")),
                (Page::About, icon::INFO, t!("settings.about")),
            ] {
                let selected = ctx.page == page;
                let btn = egui::Button::new(RichText::new(ic).size(15.0).color(if selected {
                    p.accent
                } else {
                    p.text
                }))
                .fill(if selected {
                    p.accent_soft
                } else {
                    Color32::TRANSPARENT
                })
                .stroke(Stroke::NONE)
                .min_size(egui::vec2(28.0, 28.0));
                if ui.add(btn).on_hover_text(tip).clicked() {
                    ctx.page = if selected { ctx.last_main_page } else { page };
                }
            }
            let (ic, next) = match ctx.settings.theme {
                ThemePreference::System => (icon::DESKTOP, ThemePreference::Dark),
                ThemePreference::Dark => (icon::MOON, ThemePreference::Light),
                ThemePreference::Light => (icon::SUN, ThemePreference::System),
                ThemePreference::Lime | ThemePreference::Peach => {
                    (icon::DESKTOP, ThemePreference::System)
                }
            };
            let tip = format!(
                "{}: {}",
                t!("settings.theme"),
                i18n::theme(ctx.settings.theme)
            );
            if theme::icon_button(ui, ic, &tip).clicked() {
                ctx.edit_settings(|settings| settings.theme = next);
                theme::apply_preference(gui, next);
            }
        });
    });
}

fn sub_nav(ctx: &mut Ctx<'_>, ui: &mut egui::Ui, pages: &[(Page, &str, String)]) {
    let p = theme::palette(ui);
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for &(page, ic, ref label) in pages {
            let selected = ctx.page == page;
            let (fill, fg) = if selected {
                (p.accent_soft, p.accent)
            } else {
                (Color32::TRANSPARENT, p.text)
            };
            let recording = page == ctx.recording_page() && !ctx.recording.is_idle();
            let resp = theme::marker_button(
                ui,
                theme::MarkerButton {
                    label: &format!("{ic}  {label}"),
                    font: egui::FontId::proportional(13.0),
                    color: fg,
                    fill,
                    marker: recording.then_some(p.danger),
                    height: 26.0,
                    min_width: 0.0,
                },
            );
            if resp.clicked() {
                ctx.page = page;
            }
        }
    });
}

pub fn header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    let p = theme::palette(ui);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(18.0).strong());
        if !subtitle.is_empty() {
            ui.label(RichText::new(subtitle).size(12.0).color(p.text_weak));
        }
    });
    ui.add_space(8.0);
}

pub fn tile_width(ui: &egui::Ui, n: usize) -> f32 {
    let gap = ui.spacing().item_spacing.x;
    let avail = ui.available_width();
    let cols = ((avail + gap) / (180.0 + gap)).floor().clamp(1.0, n as f32);
    ((avail - gap * (cols - 1.0)) / cols).floor()
}

pub fn tile(
    ui: &mut egui::Ui,
    ic: &str,
    title: &str,
    subtitle: &str,
    width: f32,
    selected: bool,
) -> egui::Response {
    let p = theme::palette(ui);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 62.0), egui::Sense::click());
    let hovered = resp.hovered();
    let fill = if selected {
        p.accent_soft
    } else if hovered {
        p.card_hover
    } else {
        p.card
    };
    let stroke = Stroke::new(
        if selected { 1.5 } else { 1.0 },
        if hovered || selected {
            p.accent
        } else {
            p.border
        },
    );
    ui.painter().rect(
        rect,
        CornerRadius::same(8),
        fill,
        stroke,
        egui::StrokeKind::Inside,
    );
    let badge =
        egui::Rect::from_min_size(rect.min + egui::vec2(12.0, 13.0), egui::vec2(36.0, 36.0));
    ui.painter()
        .rect_filled(badge, CornerRadius::same(7), p.accent_soft);
    ui.painter().text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        ic,
        egui::FontId::proportional(18.0),
        p.accent,
    );
    let x = badge.max.x + 10.0;
    let title_y = if subtitle.is_empty() {
        rect.center().y - 9.0
    } else {
        rect.min.y + 14.0
    };
    ui.painter().text(
        egui::pos2(x, title_y),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::proportional(14.0),
        p.text,
    );
    ui.painter().text(
        egui::pos2(x, rect.min.y + 34.0),
        egui::Align2::LEFT_TOP,
        subtitle,
        egui::FontId::proportional(11.5),
        p.text_weak,
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, title)
    });
    resp
}
