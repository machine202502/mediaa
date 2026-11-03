use eframe::egui::Color32;

use super::{IconPalette, Palette};

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

pub const ICON: IconPalette = IconPalette {
    deep: [22.0, 44.0, 120.0],
    mid: [37.0, 99.0, 235.0],
    light: [34.0, 211.0, 238.0],
};
