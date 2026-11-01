use eframe::egui;
use mediaa_core::settings::ThemePreference;

#[derive(Default)]
pub(crate) struct Caches {
    logo: Option<(ThemePreference, egui::TextureHandle)>,
    pub(crate) viewer: crate::ui::pages::library::viewer::ViewerTexture,
}

impl Caches {
    pub(crate) fn logo(
        &mut self,
        ctx: &egui::Context,
        theme: ThemePreference,
    ) -> egui::TextureHandle {
        if let Some((cached, texture)) = &self.logo {
            if *cached == theme {
                return texture.clone();
            }
        }
        let icon = crate::ui::theme::icon_palette(theme);
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [64, 64],
            &crate::icon::rgba_with(64, icon.deep, icon.mid, icon.light),
        );
        let texture = ctx.load_texture(
            format!("logo-{theme:?}"),
            image,
            egui::TextureOptions::LINEAR,
        );
        self.logo = Some((theme, texture.clone()));
        texture
    }
}
