//! Compact blue themes (dark + light): one palette per mode applied to egui's visuals,
//! plus small styled-widget helpers shared by all pages.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Visuals,
};
use mediaa_core::settings::ThemePreference;

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

pub const DARK: Palette = Palette {
    bg: Color32::from_rgb(13, 21, 34),
    chrome: Color32::from_rgb(17, 28, 45),
    card: Color32::from_rgb(22, 35, 58),
    card_hover: Color32::from_rgb(29, 45, 73),
    border: Color32::from_rgb(38, 58, 92),
    text: Color32::from_rgb(227, 235, 247),
    text_weak: Color32::from_rgb(143, 163, 192),
    accent: Color32::from_rgb(61, 139, 255),
    accent_soft: Color32::from_rgb(26, 51, 88),
    on_accent: Color32::WHITE,
    danger: Color32::from_rgb(255, 93, 108),
    success: Color32::from_rgb(47, 208, 138),
    warning: Color32::from_rgb(255, 181, 71),
};

pub const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(238, 243, 250),
    chrome: Color32::from_rgb(227, 235, 246),
    card: Color32::WHITE,
    card_hover: Color32::from_rgb(242, 246, 252),
    border: Color32::from_rgb(211, 222, 238),
    text: Color32::from_rgb(15, 30, 51),
    text_weak: Color32::from_rgb(91, 110, 138),
    accent: Color32::from_rgb(37, 99, 235),
    accent_soft: Color32::from_rgb(220, 232, 253),
    on_accent: Color32::WHITE,
    danger: Color32::from_rgb(220, 47, 69),
    success: Color32::from_rgb(18, 160, 92),
    warning: Color32::from_rgb(199, 127, 10),
};

pub fn palette(ui: &egui::Ui) -> Palette {
    if ui.visuals().dark_mode {
        DARK
    } else {
        LIGHT
    }
}

fn visuals(p: &Palette, dark: bool) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
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
        // Also the colour of `RichText::strong()` (for `active`), so keep it readable.
        state.fg_stroke = Stroke::new(1.0, p.text);
        state.corner_radius = r;
        state.expansion = 0.0;
    }
    w.open = w.hovered;
    v
}

/// Dark visuals, used by surfaces that must stay dark regardless of the theme
/// (the area-selection toolbars float over arbitrary screen content).
pub fn dark_visuals() -> Visuals {
    visuals(&DARK, true)
}

/// System fonts with the symbols the built-in ones lack (`⧸`, arrows, math in file names).
const SYMBOL_FONTS: [&str; 4] = ["Segoe UI Symbol", "Noto Sans Symbols 2", "Apple Symbols", "DejaVu Sans"];

/// The first system font of [`SYMBOL_FONTS`] found.
fn symbol_font() -> Option<Vec<u8>> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let families: Vec<fontdb::Family> = SYMBOL_FONTS.iter().map(|f| fontdb::Family::Name(f)).collect();
    let id = db.query(&fontdb::Query { families: &families, ..Default::default() })?;
    db.with_face_data(id, |data, _| data.to_vec())
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    if let Some(data) = symbol_font() {
        fonts.font_data.insert("symbols".into(), std::sync::Arc::new(egui::FontData::from_owned(data)));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push("symbols".into());
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
            (TextStyle::Heading, FontId::new(18.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
        ]
        .into();
    });
}

pub fn apply_preference(ctx: &egui::Context, pref: ThemePreference) {
    ctx.set_theme(match pref {
        ThemePreference::System => egui::ThemePreference::System,
        ThemePreference::Light => egui::ThemePreference::Light,
        ThemePreference::Dark => egui::ThemePreference::Dark,
    });
}

/// Rounded surface for grouping controls.
pub fn card(ui: &egui::Ui) -> egui::Frame {
    let p = palette(ui);
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
}

/// Section title inside a card.
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
    ui.label(egui::RichText::new(text.into()).size(11.5).color(p.text_weak));
}

fn sized(btn: egui::Button<'static>) -> egui::Button<'static> {
    btn.corner_radius(CornerRadius::same(6)).min_size(egui::vec2(0.0, 28.0))
}

/// Filled accent button for primary actions.
pub fn primary(ui: &egui::Ui, icon: &str, text: &str) -> egui::Button<'static> {
    let p = palette(ui);
    sized(
        egui::Button::new(egui::RichText::new(format!("{icon}  {text}")).color(p.on_accent).strong())
            .fill(p.accent),
    )
}

pub fn danger(ui: &egui::Ui, icon: &str, text: &str) -> egui::Button<'static> {
    let p = palette(ui);
    sized(egui::Button::new(egui::RichText::new(format!("{icon}  {text}")).color(Color32::WHITE)).fill(p.danger))
}

pub fn secondary(icon: &str, text: &str) -> egui::Button<'static> {
    sized(egui::Button::new(format!("{icon}  {text}")))
}

/// Small square icon button with a tooltip.
/// A menu entry: icon and label across the menu's width, highlighted under the pointer
/// (accent tint; red tint for `danger`) with a hand cursor.
pub fn menu_item(ui: &mut egui::Ui, icon: &str, label: &str, danger: bool) -> egui::Response {
    let p = palette(ui);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if ui.is_rect_visible(rect) {
        let color = if danger { p.danger } else { p.text };
        if resp.hovered() || resp.has_focus() {
            let tint = if danger { p.danger.gamma_multiply(0.15) } else { p.accent_soft };
            ui.painter().rect_filled(rect, CornerRadius::same(6), tint);
        }
        let font = FontId::proportional(13.5);
        ui.painter().text(rect.left_center() + egui::vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, icon, FontId::proportional(15.0), color);
        ui.painter().text(rect.left_center() + egui::vec2(36.0, 0.0), egui::Align2::LEFT_CENTER, label, font, color);
    }
    resp
}

/// Side of [`icon_button`].
pub const ICON_BUTTON: f32 = 26.0;

pub fn icon_button(ui: &mut egui::Ui, icon: &str, tooltip: &str) -> egui::Response {
    icon_button_sized(ui, icon, tooltip, ICON_BUTTON)
}

/// [`icon_button`] in a box of the given side — for rows whose height must not change when the
/// button appears (a smaller box keeps the row's geometry).
pub fn icon_button_sized(ui: &mut egui::Ui, icon: &str, tooltip: &str, side: f32) -> egui::Response {
    // Painted by hand at a fixed size: egui buttons that show their frame only on hover grow
    // by the frame stroke, which nudges the whole row sideways.
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let p = palette(ui);
        let (fill, stroke) = if resp.is_pointer_button_down_on() {
            (p.accent, p.accent)
        } else if resp.hovered() {
            (p.accent_soft, p.accent)
        } else {
            (Color32::TRANSPARENT, Color32::TRANSPARENT)
        };
        let color = if !ui.is_enabled() {
            p.text_weak
        } else if resp.is_pointer_button_down_on() {
            p.on_accent
        } else {
            p.text
        };
        ui.painter().rect(rect, CornerRadius::same(6), fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon, FontId::proportional(14.0), color);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tooltip));
    resp.on_hover_text(tooltip)
}

/// Segmented control: a row of mutually exclusive options.
pub fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, String)]) -> bool {
    let p = palette(ui);
    let mut changed = false;
    egui::Frame::new()
        .fill(p.card_hover)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (opt, label) in options {
                    let selected = *value == *opt;
                    let text = egui::RichText::new(label.as_str()).color(if selected { p.on_accent } else { p.text });
                    let btn = egui::Button::new(text)
                        .fill(if selected { p.accent } else { Color32::TRANSPARENT })
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(5))
                        .min_size(egui::vec2(48.0, 22.0));
                    if ui.add(btn).clicked() && !selected {
                        *value = *opt;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// Two-column row: fixed-width label on the left, control on the right.
pub fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let p = palette(ui);
        // Narrow columns get a narrower label column.
        let label_w = (ui.available_width() * 0.34).clamp(70.0, 118.0);
        ui.add_sized([label_w, 24.0], egui::Label::new(egui::RichText::new(label).color(p.text_weak)).truncate());
        // Wrapping, so long texts and wide controls wrap instead of widening the card.
        ui.horizontal_wrapped(|ui| add(ui)).inner
    })
    .inner
}

/// Narrowest column of a side-by-side card (label + slider + value fit); below two of them
/// the sections are stacked vertically.
pub const COLUMN_MIN: f32 = 330.0;

pub fn is_wide(ui: &egui::Ui) -> bool {
    ui.available_width() >= 2.0 * COLUMN_MIN + ui.spacing().item_spacing.x
}

/// One card with two sections: side by side when there is room (a single card, so both halves
/// share its height naturally), stacked with a separator when the window is narrow.
pub fn split_card(ui: &mut egui::Ui, left: impl FnOnce(&mut egui::Ui), right: impl FnOnce(&mut egui::Ui)) {
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


/// Two separate cards side by side with exactly the same height (that of the taller one, in
/// the same frame — both contents are laid out before either card is painted); stacked
/// vertically when the window is narrow.
pub fn equal_cards(
    ui: &mut egui::Ui,
    _id: &str,
    left: impl FnOnce(&mut egui::Ui),
    right: impl FnOnce(&mut egui::Ui),
) {
    if !is_wide(ui) {
        for add in [Box::new(left) as Box<dyn FnOnce(&mut egui::Ui)>, Box::new(right)] {
            card(ui).show(ui, |ui| {
                ui.set_width(ui.available_width());
                add(ui);
            });
            ui.add_space(4.0);
        }
        return;
    }
    ui.columns(2, |cols| {
        let (a, b) = cols.split_at_mut(1);
        let (ca, cb) = (&mut a[0], &mut b[0]);
        let mut pa = card(ca).begin(ca);
        let mut pb = card(cb).begin(cb);
        pa.content_ui.set_width(pa.content_ui.available_width());
        pb.content_ui.set_width(pb.content_ui.available_width());
        left(&mut pa.content_ui);
        right(&mut pb.content_ui);
        // Pad the shorter card (`set_min_height` would measure from the cursor, not the top).
        let (ha, hb) = (pa.content_ui.min_rect().height(), pb.content_ui.min_rect().height());
        let h = ha.max(hb);
        pa.content_ui.add_space(h - ha);
        pb.content_ui.add_space(h - hb);
        pa.end(ca);
        pb.end(cb);
    });
}

#[cfg(test)]
mod tests {
    /// Windows always has Segoe UI Symbol: file names with `⧸` render.
    #[cfg(windows)]
    #[test]
    fn a_symbol_font_is_found() {
        let data = super::symbol_font().expect("a symbol font");
        assert!(data.len() > 100_000);
    }
}
