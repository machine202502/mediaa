use crate::textures::Textures;
use crate::ui::theme;
use crate::ui_state::UiState;
use crate::view::BackendView;
use eframe::egui::{self, Color32, Margin, RichText, Stroke};
use egui_phosphor::regular as icon;
use mediaa2_core::formats::MediaKind;
use mediaa2_core::navigation::Page;
use mediaa2_core::notices::NoticeLevel;
use mediaa2_core::recording::{RecordTarget, StopAction, StopSource};
use mediaa2_core::screen::{MonitorInfo, WindowInfo};
use mediaa2_core::settings::{RecordProfile, Settings, ThemePreference};
use mediaa2_i18n::t;
use mediaa2_protocol::state::{ConversionState, RecordingState};
use mediaa2_protocol::{ClientCommand, ConversionCommand, RecordingCommand};
use std::time::Duration;

pub mod about;
pub mod audio;
pub mod capture;
pub mod convert;
pub mod library;
pub mod notices;
pub mod picker;
pub mod record;
pub mod settings;
pub mod viewer;

pub struct Ctx<'a> {
    pub view: &'a BackendView,
    pub ui: &'a mut UiState,
    pub actions: &'a mut Actions,
    pub textures: &'a mut Textures,
    pub monitors: &'a [MonitorInfo],
    pub windows: &'a [WindowInfo],
    pub microphones: &'a [String],
    pub command_error: Option<&'a str>,
}

pub struct ActionNotice {
    pub level: NoticeLevel,
    pub summary: String,
    pub detail: String,
}

#[derive(Default)]
pub struct Actions {
    commands: Vec<ClientCommand>,
    notices: Vec<ActionNotice>,
    quit: bool,
    refresh_catalogs: bool,
}

impl Actions {
    pub fn send(&mut self, command: ClientCommand) {
        self.commands.push(command);
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.commands.push(ClientCommand::SetSettings(settings));
    }

    pub fn notice(
        &mut self,
        level: NoticeLevel,
        summary: impl Into<String>,
        detail: impl Into<String>,
    ) {
        self.notices.push(ActionNotice {
            level,
            summary: summary.into(),
            detail: detail.into(),
        });
    }

    pub fn quit(&mut self) {
        self.quit = true;
    }

    pub fn refresh_catalogs(&mut self) {
        self.refresh_catalogs = true;
    }

    pub fn take_refresh_catalogs(&mut self) -> bool {
        std::mem::take(&mut self.refresh_catalogs)
    }

    pub fn take_commands(&mut self) -> Vec<ClientCommand> {
        std::mem::take(&mut self.commands)
    }

    pub fn take_notices(&mut self) -> Vec<ActionNotice> {
        std::mem::take(&mut self.notices)
    }

    pub fn quit_requested(&self) -> bool {
        self.quit
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Library,
    Capture,
    Convert,
}

const HEADER_HEIGHT: f32 = 46.0;
const SUBNAV_HEIGHT: f32 = 36.0;
const NARROW_HEADER: f32 = 820.0;
const RECORDING_PAGE: &str = "mediaa-recording-page";

pub fn section_for(page: Page) -> Option<Section> {
    match page {
        Page::Library => Some(Section::Library),
        Page::Screenshot | Page::Record | Page::RecordAudio => Some(Section::Capture),
        Page::Convert => Some(Section::Convert),
        Page::Settings | Page::About => None,
    }
}

fn sub_pages(section: Section) -> Vec<(Page, &'static str, String)> {
    match section {
        Section::Capture => vec![
            (
                Page::Screenshot,
                icon::CAMERA,
                t!("nav.screenshot").to_string(),
            ),
            (
                Page::Record,
                icon::VIDEO_CAMERA,
                t!("nav.record").to_string(),
            ),
            (
                Page::RecordAudio,
                icon::MICROPHONE,
                t!("nav.record_audio").to_string(),
            ),
        ],
        Section::Library | Section::Convert => Vec::new(),
    }
}

fn section_page(ctx: &Ctx<'_>, section: Option<Section>) -> Page {
    match section {
        Some(Section::Library) => Page::Library,
        Some(Section::Convert) => Page::Convert,
        Some(Section::Capture) | None => ctx.ui.last_capture_page,
    }
}

fn recording_target_page(target: &RecordTarget) -> Page {
    match target {
        RecordTarget::Audio => Page::RecordAudio,
        RecordTarget::Screens { .. }
        | RecordTarget::Region { .. }
        | RecordTarget::Windows { .. } => Page::Record,
    }
}

fn recording_page(ctx: &Ctx<'_>, ui: &egui::Ui) -> Page {
    let id = egui::Id::new(RECORDING_PAGE);
    match ctx.view.recording() {
        RecordingState::Starting { target, .. } | RecordingState::Active { target, .. } => {
            let page = recording_target_page(target);
            ui.ctx().data_mut(|data| data.insert_temp(id, page));
            page
        }
        RecordingState::Finalizing { .. } => ui
            .ctx()
            .data(|data| data.get_temp::<Page>(id))
            .unwrap_or(Page::Record),
        RecordingState::Idle => Page::Record,
    }
}

pub fn frame(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    chrome(ctx, ui);
    notices::show(ctx, ui);
    page(ctx, ui);
}

fn chrome(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    egui::Panel::top("header")
        .resizable(false)
        .exact_size(HEADER_HEIGHT)
        .frame(
            egui::Frame::new()
                .fill(p.chrome)
                .inner_margin(Margin::symmetric(12, 6))
                .stroke(Stroke::new(1.0, p.border)),
        )
        .show(ui, |ui| header_bar(ctx, ui));

    if let Some(section) = section_for(ctx.ui.page) {
        let pages = sub_pages(section);
        if !pages.is_empty() {
            let subnav = egui::Panel::top("subnav")
                .resizable(false)
                .exact_size(SUBNAV_HEIGHT)
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

    if ctx.ui.page == Page::Convert {
        egui::Panel::bottom("convert-start")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(18, 10)),
            )
            .show(ui, |ui| {
                convert::start_bar(ctx, ui);
            });
    }

    if matches!(ctx.ui.page, Page::Record | Page::RecordAudio) {
        let live = ctx.view.recording_active();
        egui::Panel::bottom("record-start")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(18, 10)),
            )
            .show(ui, |ui| {
                ui.add_enabled_ui(!live, |ui| {
                    if ctx.ui.page == Page::Record {
                        if let Some(target) = record::start_bar(ctx, ui) {
                            if ctx.ui.settings.dirty {
                                let settings = ctx.ui.settings.draft.clone();
                                ctx.actions.set_settings(settings);
                            }
                            ctx.actions
                                .send(ClientCommand::Recording(RecordingCommand::Start {
                                    target,
                                    profile: RecordProfile::App,
                                    hide: None,
                                }));
                        }
                    } else if audio::start_bar(ctx, ui) {
                        let settings = ctx.ui.settings.draft.clone();
                        ctx.actions.set_settings(settings);
                        ctx.actions
                            .send(ClientCommand::Recording(RecordingCommand::Start {
                                target: RecordTarget::Audio,
                                profile: RecordProfile::App,
                                hide: None,
                            }));
                    }
                });
            });
    }

    status_overlay(ctx, ui);
}

fn status_overlay(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let page = ctx.ui.page;
    let recording = ctx.view.recording().clone();
    let conversion = ctx.view.conversion().clone();
    let show = match page {
        Page::Convert => !matches!(conversion, ConversionState::Idle),
        Page::Record | Page::RecordAudio => {
            !matches!(recording, RecordingState::Idle)
                && !(matches!(recording, RecordingState::Starting { .. })
                    && ctx.ui.settings.draft.hide_recording_preparation)
        }
        _ => false,
    };
    let gui = ui.ctx().clone();
    let shown_id = egui::Id::new("status-overlay-shown");
    let was_shown = gui
        .data(|data| data.get_temp::<bool>(shown_id))
        .unwrap_or(false);
    gui.data_mut(|data| data.insert_temp(shown_id, show));
    if !show {
        return;
    }
    let width = 640.0_f32.min((gui.content_rect().width() - 36.0).max(280.0));
    let bottom = gui.content_rect().bottom() - 56.0 - 18.0;
    egui::Area::new(egui::Id::new("status-overlay"))
        .order(egui::Order::Foreground)
        .fade_in(false)
        .constrain(false)
        .sizing_pass(!was_shown)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .default_size(egui::vec2(width, 166.0))
        .fixed_pos(egui::pos2(gui.content_rect().center().x, bottom))
        .show(&gui, |ui| {
            ui.set_width(width);
            match page {
                Page::Convert => {
                    if let Some(card) = convert::status_card(ui, &conversion) {
                        if theme::status_card(ui, card).is_some() {
                            ctx.actions
                                .send(ClientCommand::Conversion(ConversionCommand::Cancel));
                        }
                    }
                }
                Page::Record | Page::RecordAudio => {
                    let clicked = record::status_card(ui, &recording)
                        .and_then(|card| theme::status_card(ui, card));
                    if let Some(index) = clicked {
                        match record::status_command(&recording, index) {
                            Some(record::StatusCommand::TogglePause) => {
                                ctx.actions
                                    .send(ClientCommand::Recording(RecordingCommand::TogglePause));
                            }
                            Some(record::StatusCommand::StopAndSave) => {
                                ctx.actions.send(ClientCommand::Recording(
                                    RecordingCommand::Stop {
                                        action: StopAction::Save,
                                        source: StopSource::Ui,
                                    },
                                ));
                            }
                            Some(record::StatusCommand::Discard) => {
                                open_confirm(ui.ctx(), "record-cancel-confirm");
                            }
                            Some(record::StatusCommand::CancelStart) => {
                                open_confirm(ui.ctx(), "record-start-cancel");
                            }
                            Some(record::StatusCommand::CancelFinalize) => {
                                open_confirm(ui.ctx(), "record-abort-save");
                            }
                            None => {}
                        }
                    }
                    match &recording {
                        RecordingState::Starting { .. } => {
                            if confirm_modal(
                                ui.ctx(),
                                "record-start-cancel",
                                &t!("record.cancel_title"),
                                &t!("record.cancel_text"),
                                &t!("record.cancel_confirm"),
                            ) {
                                ctx.actions
                                    .send(ClientCommand::Recording(RecordingCommand::CancelStart));
                            }
                        }
                        RecordingState::Active { .. } => {
                            if confirm_modal(
                                ui.ctx(),
                                "record-cancel-confirm",
                                &t!("record.cancel_title"),
                                &t!("record.cancel_text"),
                                &t!("record.cancel_confirm"),
                            ) {
                                ctx.actions.send(ClientCommand::Recording(
                                    RecordingCommand::Stop {
                                        action: StopAction::Discard,
                                        source: StopSource::Ui,
                                    },
                                ));
                            }
                        }
                        RecordingState::Finalizing { .. } => {
                            if confirm_modal(
                                ui.ctx(),
                                "record-abort-save",
                                &t!("record.abort_title"),
                                &t!("record.abort_text"),
                                &t!("record.abort_saving"),
                            ) {
                                ctx.actions.send(ClientCommand::Recording(
                                    RecordingCommand::CancelFinalize,
                                ));
                            }
                        }
                        RecordingState::Idle => {}
                    }
                }
                _ => {}
            }
        });
    gui.request_repaint_after(Duration::from_millis(250));
}

fn open_confirm(ctx: &egui::Context, id: &str) {
    ctx.memory_mut(|memory| {
        memory.data.insert_temp(egui::Id::new((id, "open")), true);
    });
}

fn confirm_modal(ctx: &egui::Context, id: &str, title: &str, text: &str, action: &str) -> bool {
    let open_id = egui::Id::new((id, "open"));
    let open = ctx
        .memory(|memory| memory.data.get_temp::<bool>(open_id))
        .unwrap_or(false);
    if !open {
        return false;
    }
    let mut confirmed = false;
    let mut dismiss = false;
    let modal = egui::Modal::new(egui::Id::new(id)).show(ctx, |ui| {
        ui.set_width(380.0);
        theme::popup_title(ui, title);
        ui.label(text);
        ui.add_space(8.0);
        let action_label = action.to_string();
        let cancel_label = t!("common.cancel").to_string();
        let responses = theme::centered_buttons(
            ui,
            &[
                theme::ModalButton {
                    kind: theme::ModalButtonKind::Danger,
                    icon: icon::TRASH,
                    label: &action_label,
                    enabled: true,
                },
                theme::ModalButton {
                    kind: theme::ModalButtonKind::Secondary,
                    icon: icon::X,
                    label: &cancel_label,
                    enabled: true,
                },
            ],
        );
        confirmed = responses[0].clicked();
        dismiss = responses[1].clicked();
    });
    if modal.should_close() || dismiss || confirmed {
        ctx.memory_mut(|memory| memory.data.insert_temp(open_id, false));
    }
    confirmed
}

fn header_bar(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    ui.horizontal_centered(|ui| {
        let logo = ctx.textures.app_icon(ui.ctx(), crate::dev_agent());
        ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(24.0, 24.0)));
        ui.label(RichText::new("MediaA").size(15.0).strong().color(p.text));
        ui.add_space(14.0);

        let mut section = section_for(ctx.ui.page);
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
        ];
        let mut marks: Vec<(Option<Section>, Color32)> = Vec::new();
        if ctx.view.recording_active() {
            marks.push((Some(Section::Capture), p.danger));
        }
        if ctx.view.conversion_busy() {
            marks.push((Some(Section::Convert), p.success));
        }
        if ui.ctx().content_rect().width() < NARROW_HEADER {
            let last = section_for(ctx.ui.last_main_page).unwrap_or(Section::Library);
            narrow_nav(ui, &mut section, &nav, &marks, last);
        } else {
            theme::segmented_marked_many(ui, &mut section, &nav, &marks);
        }
        if section != before {
            let page = section_page(ctx, section);
            ctx.ui.select_page(page);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (page, ic, tip) in [
                (Page::Settings, icon::GEAR, t!("nav.settings")),
                (Page::About, icon::INFO, t!("settings.about")),
            ] {
                let selected = ctx.ui.page == page;
                let button = egui::Button::new(RichText::new(ic).size(15.0).color(if selected {
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
                if ui.add(button).on_hover_text(tip).clicked() {
                    let page = if selected {
                        ctx.ui.last_main_page
                    } else {
                        page
                    };
                    ctx.ui.select_page(page);
                }
            }
            let current = ctx.view.settings().theme;
            let (theme_ic, next) = next_theme(current);
            let tip = format!("{}: {}", t!("settings.theme"), theme_label(current));
            if theme::icon_button(ui, theme_ic, &tip).clicked() {
                let mut draft = ctx.ui.settings.draft.clone();
                draft.theme = next;
                ctx.ui.settings.draft = draft.clone();
                ctx.ui.settings_draft_changed();
                ctx.actions.set_settings(draft);
                theme::apply_preference(ui.ctx(), next);
            }
        });
    });
}

fn narrow_nav(
    ui: &mut egui::Ui,
    section: &mut Option<Section>,
    nav: &[(Option<Section>, String)],
    marks: &[(Option<Section>, Color32)],
    last: Section,
) {
    let current = *section;
    let shown = current.unwrap_or(last);
    let selected = nav
        .iter()
        .find(|(option, _)| *option == Some(shown))
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
                    .layout_no_wrap(label.clone(), font.clone(), Color32::WHITE)
                    .size()
                    .x
            })
        })
        .fold(0.0_f32, f32::max);
    let marked = marks
        .iter()
        .find(|(option, _)| *option == Some(shown))
        .map(|(_, color)| *color);
    let selected_text = if marked.is_some() {
        format!("\u{00A0}\u{00A0}\u{00A0}\u{00A0}{selected}")
    } else {
        selected.clone()
    };
    let width = text_width + 34.0 + if marked.is_some() { 22.0 } else { 0.0 };
    if current.is_none() {
        let p = theme::palette(ui);
        let button = egui::Button::new(format!("{selected_text}  {}", icon::CARET_DOWN))
            .min_size(egui::vec2(width, 24.0))
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border));
        let response = ui.add(button);
        if response.clicked() {
            *section = Some(last);
        }
        if let Some(color) = marked {
            ui.painter().circle_filled(
                response.rect.left_center() + egui::vec2(9.0, 0.0),
                3.0,
                color,
            );
        }
        return;
    }
    let combo = theme::combo_box("header-section")
        .width(width)
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            for (option, label) in nav {
                ui.horizontal(|ui| {
                    if let Some(color) = marks
                        .iter()
                        .find(|(mark, _)| mark == option)
                        .map(|(_, color)| *color)
                    {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 3.0, color);
                    }
                    ui.selectable_value(section, *option, label);
                });
            }
        });
    if let Some(color) = marked {
        ui.painter().circle_filled(
            combo.response.rect.left_center() + egui::vec2(9.0, 0.0),
            3.0,
            color,
        );
    }
}

fn sub_nav(ctx: &mut Ctx<'_>, ui: &mut egui::Ui, pages: &[(Page, &str, String)]) {
    let p = theme::palette(ui);
    let recording_page = recording_page(ctx, ui);
    let recording = ctx.view.recording_active();
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for &(page, ic, ref label) in pages {
            let selected = ctx.ui.page == page;
            let (fill, fg) = if selected {
                (p.accent_soft, p.accent)
            } else {
                (Color32::TRANSPARENT, p.text)
            };
            let response = theme::marker_button(
                ui,
                theme::MarkerButton {
                    label: &format!("{ic}  {label}"),
                    font: egui::FontId::proportional(13.0),
                    color: fg,
                    fill,
                    marker: (recording && page == recording_page).then_some(p.danger),
                    height: 26.0,
                    min_width: 0.0,
                },
            );
            if response.clicked() {
                ctx.ui.select_page(page);
            }
        }
    });
}

fn next_theme(current: ThemePreference) -> (&'static str, ThemePreference) {
    match current {
        ThemePreference::System => (icon::DESKTOP, ThemePreference::Dark),
        ThemePreference::Dark => (icon::MOON, ThemePreference::Light),
        ThemePreference::Light => (icon::SUN, ThemePreference::System),
        ThemePreference::Lime | ThemePreference::Peach => (icon::DESKTOP, ThemePreference::System),
    }
}

fn theme_label(theme: ThemePreference) -> String {
    match theme {
        ThemePreference::System => t!("theme.system").to_string(),
        ThemePreference::Light => t!("theme.light").to_string(),
        ThemePreference::Dark => t!("theme.dark").to_string(),
        ThemePreference::Lime => t!("theme.lime").to_string(),
        ThemePreference::Peach => t!("theme.peach").to_string(),
    }
}

fn page(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(p.bg)
                .inner_margin(egui::Margin::symmetric(18, 14)),
        )
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(format!("page-scroll-{:?}", ctx.ui.page))
                .auto_shrink([false, false])
                .show(ui, |ui| match ctx.ui.page {
                    Page::Library => library::show(ctx, ui),
                    Page::Screenshot => capture::show(ctx, ui),
                    Page::Record => record::show(ctx, ui),
                    Page::RecordAudio => audio::show(ctx, ui),
                    Page::Convert => convert::show(ctx, ui),
                    Page::Settings => settings::show(ctx, ui),
                    Page::About => about::show(ctx, ui),
                });
        });
}

pub fn page_title(ui: &mut egui::Ui, title: &str, hint: &str) {
    let p = theme::palette(ui);
    ui.add_space(4.0);
    ui.label(egui::RichText::new(title).size(18.0).strong().color(p.text));
    if !hint.is_empty() {
        ui.label(egui::RichText::new(hint).size(12.0).color(p.text_weak));
    }
    ui.add_space(8.0);
}

pub fn backend_notices(view: &BackendView) -> &[mediaa2_protocol::state::Notice] {
    &view.state.notices
}

pub fn notice_color(level: NoticeLevel, ui: &egui::Ui) -> egui::Color32 {
    let p = theme::palette(ui);
    match level {
        NoticeLevel::Info => p.accent,
        NoticeLevel::Success => p.success,
        NoticeLevel::Warning => p.warning,
        NoticeLevel::Error => p.danger,
    }
}

pub fn kind_label(kind: MediaKind) -> String {
    match kind {
        MediaKind::Image => t!("kind.image").to_string(),
        MediaKind::Video => t!("kind.video").to_string(),
        MediaKind::Animation => t!("kind.animation").to_string(),
        MediaKind::Audio => t!("kind.audio").to_string(),
    }
}
