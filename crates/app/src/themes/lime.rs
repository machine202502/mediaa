use eframe::egui::Color32;

use super::{IconPalette, Palette};

pub const LIME: Palette = Palette {
    bg: Color32::from_rgb(243, 248, 236),
    chrome: Color32::from_rgb(230, 240, 216),
    card: Color32::from_rgb(252, 254, 248),
    card_hover: Color32::from_rgb(240, 247, 229),
    border: Color32::from_rgb(207, 226, 183),
    text: Color32::from_rgb(30, 51, 21),
    text_weak: Color32::from_rgb(96, 115, 79),
    accent: Color32::from_rgb(114, 176, 29),
    accent_soft: Color32::from_rgb(226, 240, 201),
    on_accent: Color32::WHITE,
    danger: Color32::from_rgb(198, 56, 75),
    success: Color32::from_rgb(26, 138, 74),
    warning: Color32::from_rgb(184, 121, 18),
};

pub const ICON: IconPalette = IconPalette {
    deep: [39.0, 72.0, 18.0],
    mid: [114.0, 176.0, 29.0],
    light: [196.0, 228.0, 112.0],
};
