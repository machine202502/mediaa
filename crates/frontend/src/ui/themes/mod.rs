pub use crate::ui::widgets::button::{button_width, danger, primary, secondary};
pub use crate::ui::widgets::icon_button::{
    framed_icon_button, icon_button, icon_button_sized, icon_face, ICON_BUTTON,
};
pub use crate::ui::widgets::layout::{centered_row, centered_row_sized, centered_widget};
pub use crate::ui::widgets::marker::{marker_button, MarkerButton};
pub use crate::ui::widgets::menu_item::menu_item;
pub use crate::ui::widgets::modal::{
    centered_buttons, popup_title, ModalButton, ModalButtonKind, POPUP_TITLE,
};
pub use crate::ui::widgets::segmented::{segmented, segmented_marked_many};
pub use crate::ui::widgets::status_card::{status_card, StatusCard};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Visuals,
};
use egui_phosphor::regular as icon;
use mediaa_core::settings::ThemePreference;

mod dark;
mod light;
mod lime;
mod peach;

pub use dark::DARK;
pub use light::LIGHT;
pub use lime::LIME;
pub use peach::PEACH;

#[derive(Debug, Clone, Copy)]
pub struct IconPalette {
    pub deep: [f32; 3],
    pub mid: [f32; 3],
    pub light: [f32; 3],
}

pub fn icon_palette(pref: ThemePreference) -> IconPalette {
    match pref {
        ThemePreference::Lime => lime::ICON,
        ThemePreference::Peach => peach::ICON,
        ThemePreference::Light => light::ICON,
        ThemePreference::System | ThemePreference::Dark => dark::ICON,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub chrome: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
}

fn palette_for(visuals: &Visuals) -> Palette {
    if visuals.panel_fill == LIME.bg {
        LIME
    } else if visuals.panel_fill == PEACH.bg {
        PEACH
    } else if visuals.dark_mode {
        DARK
    } else {
        LIGHT
    }
}

pub fn palette(ui: &egui::Ui) -> Palette {
    palette_for(ui.visuals())
}

pub fn current_palette(ctx: &egui::Context) -> Palette {
    let style = ctx.style_of(ctx.theme());
    palette_for(&style.visuals)
}

pub fn preview_loader(ui: &egui::Ui, center: egui::Pos2, size: f32) {
    let p = palette(ui);
    let t = ui.input(|input| input.time) as f32;
    let painter = ui.painter();
    let orbit = size * 0.28;
    for i in 0..3 {
        let angle = t * 2.8 + i as f32 * std::f32::consts::TAU / 3.0;
        let alpha = (90.0 + 55.0 * (angle - t * 2.8).cos()) as u8;
        let color =
            Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), alpha);
        painter.circle_filled(
            center + egui::vec2(angle.cos() * orbit, angle.sin() * orbit),
            size * 0.085,
            color,
        );
    }
    painter.circle_filled(center, size * 0.035, p.accent);
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(16));
}

pub const TAB_SCROLL_BAR: f32 = 3.0;

pub fn tab_row_scroll(ui: &mut egui::Ui) {
    let p = palette(ui);
    let mut scroll = egui::style::ScrollStyle::solid();
    scroll.bar_width = TAB_SCROLL_BAR;
    scroll.bar_inner_margin = 0.0;
    scroll.bar_outer_margin = 0.0;
    scroll.handle_min_length = 12.0;
    scroll.foreground_color = false;
    ui.style_mut().spacing.scroll = scroll;
    let visuals = ui.visuals_mut();
    let radius = CornerRadius::same(2);
    visuals.widgets.inactive.bg_fill = p.text_weak;
    visuals.widgets.inactive.corner_radius = radius;
    visuals.widgets.hovered.corner_radius = radius;
    visuals.widgets.active.corner_radius = radius;
    visuals.extreme_bg_color = p.border;
}

fn visuals(p: &Palette, dark: bool) -> Visuals {
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let r = CornerRadius::same(6);
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.extreme_bg_color = if dark { p.chrome } else { Color32::WHITE };
    v.faint_bg_color = p.card_hover;
    v.code_bg_color = p.card_hover;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0, p.on_accent);
    v.error_fg_color = p.danger;
    v.warn_fg_color = p.warning;
    v.override_text_color = Some(p.text);
    v.slider_trailing_fill = true;

    let w = &mut v.widgets;
    for (state, fill, stroke) in [
        (&mut w.noninteractive, p.card, p.border),
        (&mut w.inactive, p.card_hover, p.border),
        (&mut w.hovered, p.accent_soft, p.accent),
        (&mut w.active, p.accent, p.accent),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.0, p.text);
        state.corner_radius = r;
        state.expansion = 0.0;
    }
    w.open = w.hovered;
    v
}

const SYMBOL_FONTS: [&str; 4] = [
    "Segoe UI Symbol",
    "Noto Sans Symbols 2",
    "Apple Symbols",
    "DejaVu Sans",
];

fn symbol_font() -> Option<Vec<u8>> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let families: Vec<fontdb::Family> = SYMBOL_FONTS
        .iter()
        .map(|f| fontdb::Family::Name(f))
        .collect();
    let id = db.query(&fontdb::Query {
        families: &families,
        ..Default::default()
    })?;
    db.with_face_data(id, |data, _| data.to_vec())
}

pub fn combo_box(id_salt: impl egui::AsIdSalt) -> egui::ComboBox {
    egui::ComboBox::from_id_salt(id_salt).icon(|ui, rect, visuals, open| {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            if open {
                icon::CARET_UP
            } else {
                icon::CARET_DOWN
            },
            FontId::proportional(14.0),
            visuals.text_color(),
        );
    })
}
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    if let Some(data) = symbol_font() {
        fonts.font_data.insert(
            "symbols".into(),
            std::sync::Arc::new(egui::FontData::from_owned(data)),
        );
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("symbols".into());
        }
    }
    ctx.set_fonts(fonts);

    ctx.set_visuals_of(egui::Theme::Dark, visuals(&DARK, true));
    ctx.set_visuals_of(egui::Theme::Light, visuals(&LIGHT, false));
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 3.0);
        style.spacing.interact_size.y = 24.0;
        style.spacing.slider_width = 170.0;
        style.spacing.combo_width = 190.0;
        style.text_styles = [
            (
                TextStyle::Heading,
                FontId::new(18.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(13.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Small,
                FontId::new(11.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(12.0, FontFamily::Monospace),
            ),
        ]
        .into();
    });
}

pub fn apply_preference(ctx: &egui::Context, pref: ThemePreference) {
    ctx.set_visuals_of(egui::Theme::Light, visuals(&LIGHT, false));
    ctx.set_visuals_of(egui::Theme::Dark, visuals(&DARK, true));
    ctx.set_theme(match pref {
        ThemePreference::System => egui::ThemePreference::System,
        ThemePreference::Light | ThemePreference::Lime | ThemePreference::Peach => {
            egui::ThemePreference::Light
        }
        ThemePreference::Dark => egui::ThemePreference::Dark,
    });
    match pref {
        ThemePreference::Lime => {
            ctx.set_visuals_of(egui::Theme::Light, visuals(&LIME, false));
        }
        ThemePreference::Peach => {
            ctx.set_visuals_of(egui::Theme::Light, visuals(&PEACH, false));
        }
        _ => {}
    }
}

pub fn card(ui: &egui::Ui) -> egui::Frame {
    let p = palette(ui);
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
}

pub fn section(ui: &mut egui::Ui, icon: &str, title: &str) {
    let p = palette(ui);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(icon).size(15.0).color(p.accent));
        ui.label(egui::RichText::new(title).size(14.0).strong());
    });
    ui.add_space(2.0);
}

pub fn hint(ui: &mut egui::Ui, text: impl Into<String>) {
    let p = palette(ui);
    ui.label(
        egui::RichText::new(text.into())
            .size(11.5)
            .color(p.text_weak),
    );
}

pub fn with_compact_title<R>(ctx: &egui::Context, f: impl FnOnce() -> R) -> R {
    let mut previous = FontId::new(18.0, FontFamily::Proportional);
    ctx.all_styles_mut(|style| {
        if let Some(font) = style.text_styles.get(&TextStyle::Heading) {
            previous = font.clone();
        }
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(POPUP_TITLE, FontFamily::Proportional),
        );
    });
    let _restore = RestoreHeading { ctx, previous };
    f()
}

struct RestoreHeading<'a> {
    ctx: &'a egui::Context,
    previous: FontId,
}

impl Drop for RestoreHeading<'_> {
    fn drop(&mut self) {
        let previous = self.previous.clone();
        self.ctx.all_styles_mut(|style| {
            style
                .text_styles
                .insert(TextStyle::Heading, previous.clone());
        });
    }
}

pub fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let p = palette(ui);
        let label_w = (ui.available_width() * 0.34).clamp(70.0, 118.0);
        ui.add_sized(
            [label_w, 24.0],
            egui::Label::new(egui::RichText::new(label).color(p.text_weak)).truncate(),
        );
        ui.horizontal_wrapped(|ui| add(ui)).inner
    })
    .inner
}

pub const COLUMN_MIN: f32 = 330.0;

pub fn is_wide(ui: &egui::Ui) -> bool {
    ui.available_width() >= 2.0 * COLUMN_MIN + ui.spacing().item_spacing.x
}

pub fn split_card(
    ui: &mut egui::Ui,
    left: impl FnOnce(&mut egui::Ui),
    right: impl FnOnce(&mut egui::Ui),
) {
    card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if is_wide(ui) {
            ui.columns(2, |cols| {
                left(&mut cols[0]);
                right(&mut cols[1]);
            });
        } else {
            left(ui);
            ui.separator();
            right(ui);
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn tab_row_scroll_is_thin_solid_and_muted() {
        let _ui = crate::test_support::ui_test_guard();
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            super::tab_row_scroll(ui);
            let scroll = ui.spacing().scroll;
            assert!(!scroll.floating);
            assert_eq!(scroll.bar_width, super::TAB_SCROLL_BAR);
            assert_eq!(scroll.allocated_width(), super::TAB_SCROLL_BAR);
            let p = super::palette(ui);
            assert_eq!(ui.visuals().widgets.inactive.bg_fill, p.text_weak);
            assert_eq!(ui.visuals().extreme_bg_color, p.border);
        });
        harness.run();
    }

    #[cfg(windows)]
    #[test]
    fn a_symbol_font_is_found() {
        let data = super::symbol_font().expect("a symbol font");
        assert!(data.len() > 100_000);
    }

    #[test]
    fn custom_preference_applies_its_palette_and_visuals() {
        let _ui = crate::test_support::ui_test_guard();
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, frame: &mut u8| {
                if *frame == 0 {
                    super::apply_preference(ui.ctx(), mediaa_core::settings::ThemePreference::Lime);
                    *frame = 1;
                    return;
                }
                let p = super::palette(ui);
                assert_eq!(p.bg, super::LIME.bg);
                assert_eq!(ui.visuals().panel_fill, super::LIME.bg);
                assert_eq!(ui.visuals().widgets.active.bg_fill, super::LIME.accent);
            },
            0_u8,
        );
        harness.run_steps(2);
    }
}
