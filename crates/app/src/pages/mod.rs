pub mod about;
pub mod agent;
pub mod audio;
pub mod capture;
pub mod record;
pub mod settings;

use crate::app::command::{AppCommand, RecordingCommand};
use crate::app::state::Page;
use crate::app::App;
use crate::features::library;
use crate::overlay;
use crate::{i18n, theme};
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};
use egui_phosphor::regular as icon;
use mediaa_core::library::human_duration;
use mediaa_core::settings::{RecordProfile, ThemePreference};
use mediaa_media::recorder::StopAction;
use rust_i18n::t;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Library,
    Capture,
    Convert,
    Agent,
}

impl Page {
    pub fn section(self) -> Option<Section> {
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

pub fn shell(app: &mut App, ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    let ctx = ui.ctx().clone();
    egui::Panel::top("header")
        .resizable(false)
        .exact_size(46.0)
        .frame(
            egui::Frame::new()
                .fill(p.chrome)
                .inner_margin(Margin::symmetric(12, 6))
                .stroke(Stroke::new(1.0, p.border)),
        )
        .show(ui, |ui| header_bar(app, ui, &ctx));

    if let Some(section) = app.page.section() {
        app.last_main_page = app.page;
        match section {
            Section::Capture => app.last_capture_page = app.page,
            Section::Agent => app.last_agent_page = app.page,
            Section::Library | Section::Convert => {}
        }
        let pages = sub_pages(section);
        if !pages.is_empty() {
            egui::Panel::top("subnav")
                .resizable(false)
                .exact_size(36.0)
                .frame(
                    egui::Frame::new()
                        .fill(p.chrome)
                        .inner_margin(Margin::symmetric(12, 4))
                        .stroke(Stroke::new(1.0, p.border)),
                )
                .show(ui, |ui| sub_nav(app, ui, &pages));
        }
    }

    if matches!(app.page, Page::Record | Page::RecordAudio) && app.recording.is_idle() {
        egui::Panel::bottom("record-start")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(18, 10)),
            )
            .show(ui, |ui| {
                if app.page == Page::Record {
                    let target = app.record_target_from_ui().map(|_| ());
                    if record::start_bar(&app.settings, app.recovery_active(), &target, ui) {
                        app.start_recording_from_ui(ui.ctx());
                    }
                } else {
                    if audio::start_bar(&app.settings, app.recovery_active(), ui) {
                        app.start_audio_recording(ui.ctx(), RecordProfile::App);
                    }
                }
            });
    }

    if app.page == Page::Catalogs {
        agent::catalogs::screen(app, ui);
        return;
    }

    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(p.bg)
                .inner_margin(Margin::symmetric(18, 14)),
        )
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match app.page {
                    Page::Screenshot => {
                        let actions =
                            capture::ui(&mut app.settings, &app.monitors, &app.windows, ui);
                        for action in actions {
                            match action {
                                capture::CaptureUiAction::SettingsChanged => app.settings_changed(),
                                capture::CaptureUiAction::RefreshCatalogs => {
                                    app.refresh_catalogs(ui.ctx())
                                }
                                capture::CaptureUiAction::StartRegion => app
                                    .start_region_screenshot(
                                        ui.ctx(),
                                        overlay::Purpose::Screenshot,
                                    ),
                                capture::CaptureUiAction::StartFullscreen(monitor) => {
                                    app.start_fullscreen_screenshot(ui.ctx(), monitor)
                                }
                                capture::CaptureUiAction::StartWindow(id) => {
                                    app.start_window_screenshot(ui.ctx(), id)
                                }
                            }
                        }
                    }
                    Page::Record => {
                        let actions = record::ui(
                            &mut app.recording,
                            &mut app.settings,
                            &app.monitors,
                            &app.windows,
                            &app.mic_devices,
                            ui,
                        );
                        for action in actions {
                            match action {
                                record::RecordUiAction::SettingsChanged => app.settings_changed(),
                                record::RecordUiAction::RefreshCatalogs => {
                                    app.refresh_catalogs(ui.ctx())
                                }
                                record::RecordUiAction::SelectRegion => app
                                    .start_region_screenshot(
                                        ui.ctx(),
                                        overlay::Purpose::RecordRegion,
                                    ),
                                record::RecordUiAction::RecordingCommand(command) => {
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
                                    app.dispatch_command(ui.ctx(), AppCommand::Recording(command));
                                }
                            }
                        }
                    }
                    Page::RecordAudio => {
                        let actions =
                            audio::ui(&mut app.recording, &mut app.settings, &app.mic_devices, ui);
                        for action in actions {
                            match action {
                                audio::AudioUiAction::SettingsChanged => app.settings_changed(),
                                audio::AudioUiAction::RefreshCatalogs => {
                                    app.refresh_catalogs(ui.ctx())
                                }
                                audio::AudioUiAction::RecordingCommand(command) => {
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
                                    app.dispatch_command(ui.ctx(), AppCommand::Recording(command));
                                }
                            }
                        }
                    }
                    Page::Library => {
                        library::dialogs(app, ui.ctx());
                        let actions = app.lib.ui(&app.settings, &mut app.thumbs, ui);
                        for action in actions {
                            library::apply_action(app, ui.ctx(), action);
                        }
                    }
                    Page::Convert => {
                        if let Some(action) = app.conversion.ui(ui, &app.settings.convert_dir()) {
                            app.apply_conversion_ui_action(ui.ctx(), action);
                        }
                    }
                    Page::Providers => agent::providers::ui(app, ui),
                    Page::Models => agent::models::ui(app, ui),
                    Page::Queue => agent::queue::ui(app, ui),
                    Page::Budget => agent::budget::ui(app, ui),
                    Page::Catalogs => {}
                    Page::Settings => settings::ui(app, ui),
                    Page::About => about::ui(app, ui),
                });
        });
}

fn header_bar(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    let p = theme::palette(ui);
    ui.horizontal_centered(|ui| {
        let logo = app.logo(ctx);
        ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(24.0, 24.0)));
        ui.label(RichText::new("MediaA").size(15.0).strong());
        ui.add_space(14.0);

        let mut section = app.page.section();
        let before = section;
        theme::segmented(
            ui,
            &mut section,
            &[
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
            ],
        );
        if section != before {
            app.page = match section {
                Some(Section::Library) => Page::Library,
                Some(Section::Capture) | None => app.last_capture_page,
                Some(Section::Convert) => Page::Convert,
                Some(Section::Agent) => app.last_agent_page,
            };
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (page, ic, tip) in [
                (Page::Settings, icon::GEAR, t!("nav.settings")),
                (Page::About, icon::INFO, t!("settings.about")),
            ] {
                let selected = app.page == page;
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
                    app.page = if selected { app.last_main_page } else { page };
                }
            }
            let (ic, next) = match app.settings.theme {
                ThemePreference::System => (icon::DESKTOP, ThemePreference::Dark),
                ThemePreference::Dark => (icon::MOON, ThemePreference::Light),
                ThemePreference::Light => (icon::SUN, ThemePreference::System),
            };
            let tip = format!(
                "{}: {}",
                t!("settings.theme"),
                i18n::theme(app.settings.theme)
            );
            if theme::icon_button(ui, ic, &tip).clicked() {
                app.settings.theme = next;
                theme::apply_preference(ctx, next);
                app.settings_changed();
            }
            recording_status(app, ui, ctx);
        });
    });
}

fn sub_nav(app: &mut App, ui: &mut egui::Ui, pages: &[(Page, &str, String)]) {
    let p = theme::palette(ui);
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for &(page, ic, ref label) in pages {
            let selected = app.page == page;
            let (fill, fg) = if selected {
                (p.accent_soft, p.accent)
            } else {
                (Color32::TRANSPARENT, p.text)
            };
            let recording = page == app.recording_page() && !app.recording.is_idle();
            let text = if recording {
                format!("{ic}  {label}    ")
            } else {
                format!("{ic}  {label}")
            };
            let btn = egui::Button::new(RichText::new(text).size(13.0).color(fg))
                .fill(fill)
                .stroke(Stroke::NONE)
                .corner_radius(CornerRadius::same(6))
                .min_size(egui::vec2(0.0, 26.0));
            let resp = ui.add(btn);
            if resp.clicked() {
                app.page = page;
            }
            if recording {
                let c = resp.rect.right_center() - egui::vec2(10.0, 0.0);
                ui.painter().circle_filled(c, 3.5, p.danger);
            }
        }
    });
}

fn recording_status(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    let p = theme::palette(ui);
    let recording = &app.recording;
    let (label, paused) = if let Some(elapsed) = recording.starting_elapsed() {
        (
            format!(
                "{} {}",
                t!("record.starting"),
                human_duration(elapsed.as_secs_f64())
            ),
            false,
        )
    } else if let Some(elapsed) = recording.recording_elapsed() {
        (
            format!(
                "{} {}",
                if recording.is_paused() {
                    t!("record.paused")
                } else {
                    t!("record.recording")
                },
                human_duration(elapsed.as_secs_f64())
            ),
            recording.is_paused(),
        )
    } else if let Some(progress) = recording.progress() {
        (
            format!("{} {:.0}%", t!("record.saving"), progress * 100.0),
            false,
        )
    } else if let Some(started) = recording.saving_started() {
        (
            format!(
                "{} {}",
                t!("record.saving"),
                human_duration(started.elapsed().as_secs_f64())
            ),
            false,
        )
    } else {
        return;
    };
    if recording.is_recording() {
        if theme::icon_button(ui, icon::X, &t!("record.cancel")).clicked() {
            app.dispatch_command(
                ctx,
                AppCommand::Recording(RecordingCommand::Stop {
                    action: StopAction::Discard,
                    source: crate::features::recording::StopSource::Ui,
                }),
            );
        }
        if theme::icon_button(ui, icon::STOP, &t!("record.stop_save")).clicked() {
            app.dispatch_command(
                ctx,
                AppCommand::Recording(RecordingCommand::Stop {
                    action: StopAction::Save,
                    source: crate::features::recording::StopSource::Ui,
                }),
            );
        }
        let (ic, tip) = if paused {
            (icon::PLAY, t!("record.resume"))
        } else {
            (icon::PAUSE, t!("record.pause"))
        };
        if theme::icon_button(ui, ic, &tip).clicked() {
            app.dispatch_command(ctx, AppCommand::Recording(RecordingCommand::TogglePause));
        }
    }
    ui.label(RichText::new(label).strong());
    ui.label(RichText::new(icon::RECORD).color(if paused { p.warning } else { p.danger }));
    ui.add_space(8.0);
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

pub fn activity(ui: &mut egui::Ui, label: &str, elapsed: Duration) {
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().size(18.0));
        ui.label(label);
        ui.monospace(human_duration(elapsed.as_secs_f64()));
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_row_does_not_take_the_remaining_page_height() {
        let ctx = egui::Context::default();
        let mut height = 0.0;
        let output = ctx.run_ui(Default::default(), |ui| {
            ui.set_min_height(600.0);
            let top = ui.cursor().min.y;
            activity(ui, "", Duration::from_secs(8));
            height = ui.cursor().min.y - top;
        });
        output.drop_without_applying_deltas();
        assert!(height < 40.0, "{height}");
    }
}
