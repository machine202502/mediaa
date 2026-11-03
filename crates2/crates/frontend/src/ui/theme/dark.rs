use eframe::egui::Color32;

use super::{IconPalette, Palette};

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

pub const ICON: IconPalette = IconPalette {
    deep: [22.0, 44.0, 120.0],
    mid: [37.0, 99.0, 235.0],
    light: [34.0, 211.0, 238.0],
};
