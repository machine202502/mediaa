use eframe::egui::Color32;

use super::{IconPalette, Palette};

pub const PEACH: Palette = Palette {
    bg: Color32::from_rgb(255, 245, 239),
    chrome: Color32::from_rgb(251, 231, 218),
    card: Color32::from_rgb(255, 253, 251),
    card_hover: Color32::from_rgb(255, 240, 230),
    border: Color32::from_rgb(242, 205, 184),
    text: Color32::from_rgb(58, 36, 27),
    text_weak: Color32::from_rgb(140, 108, 92),
    accent: Color32::from_rgb(240, 122, 75),
    accent_soft: Color32::from_rgb(251, 224, 209),
    on_accent: Color32::WHITE,
    danger: Color32::from_rgb(214, 67, 94),
    success: Color32::from_rgb(42, 148, 98),
    warning: Color32::from_rgb(194, 122, 22),
};

pub const ICON: IconPalette = IconPalette {
    deep: [126.0, 47.0, 30.0],
    mid: [240.0, 122.0, 75.0],
    light: [255.0, 193.0, 148.0],
};
