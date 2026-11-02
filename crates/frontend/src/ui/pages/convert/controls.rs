use crate::ui::theme;
use eframe::egui;
use mediaa_core::limits::VIDEO_QUALITY;
use mediaa_core::settings::AUDIO_BITRATES;
use mediaa_media::convert::{Effort, PNG_COLORS, VIDEO_FPS};
use rust_i18n::t;

pub(super) fn effort_row(ui: &mut egui::Ui, effort: &mut Effort) {
    theme::row(ui, &t!("convert.effort"), |ui| {
        let opts: Vec<(Effort, String)> = Effort::ALL
            .iter()
            .map(|e| {
                let label = match e {
                    Effort::Fastest => t!("convert.effort_fastest"),
                    Effort::Fast => t!("convert.effort_fast"),
                    Effort::Balanced => t!("convert.effort_balanced"),
                    Effort::Max => t!("convert.effort_max"),
                };
                (*e, label.to_string())
            })
            .collect();
        ui.scope(|ui| theme::segmented(ui, effort, &opts))
            .response
            .on_hover_text(t!("convert.effort_hint"));
    });
}

pub(super) fn kbps_combo(ui: &mut egui::Ui, kbps: &mut u32) {
    crate::ui::theme::combo_box("convert-kbps")
        .selected_text(format!("{kbps} kbit/s"))
        .show_ui(ui, |ui| {
            for k in AUDIO_BITRATES {
                ui.selectable_value(kbps, k, format!("{k} kbit/s"));
            }
        });
}

pub(super) fn mono_checkbox(ui: &mut egui::Ui, mono: &mut bool) {
    ui.checkbox(mono, t!("convert.mono"))
        .on_hover_text(t!("convert.mono_hint"));
}

pub(super) fn fps_combo(ui: &mut egui::Ui, max_fps: &mut Option<u32>, source_fps: f64) {
    let label = |f: Option<u32>| {
        f.map_or_else(
            || t!("convert.fps_original").to_string(),
            |f| format!("{f} fps"),
        )
    };
    crate::ui::theme::combo_box("convert-fps")
        .selected_text(label(*max_fps))
        .show_ui(ui, |ui| {
            ui.selectable_value(max_fps, None, label(None));
            for f in VIDEO_FPS
                .into_iter()
                .filter(|f| source_fps <= 0.0 || f64::from(*f) < source_fps - 0.5)
            {
                ui.selectable_value(max_fps, Some(f), label(Some(f)));
            }
        })
        .response
        .on_hover_text(t!("convert.fps_hint"));
}

pub(super) fn png_colors_combo(ui: &mut egui::Ui, colors: &mut Option<u16>) {
    let label = |c: Option<u16>| {
        c.map_or_else(
            || t!("convert.colors_all").to_string(),
            |n| t!("convert.colors_n", n = n).to_string(),
        )
    };
    crate::ui::theme::combo_box("convert-colors")
        .selected_text(label(*colors))
        .show_ui(ui, |ui| {
            ui.selectable_value(colors, None, label(None));
            for n in PNG_COLORS {
                ui.selectable_value(colors, Some(n), label(Some(n)));
            }
        })
        .response
        .on_hover_text(t!("convert.colors_hint"));
}

pub(super) fn quality_row(ui: &mut egui::Ui, quality: &mut u8) {
    theme::row(ui, &t!("record.quality"), |ui| {
        ui.add(
            egui::Slider::new(quality, VIDEO_QUALITY.min as u8..=VIDEO_QUALITY.max as u8)
                .suffix("%"),
        );
    });
}

pub(super) fn pick_file_dialog(
    controller: &mut crate::features::conversion::ConversionController,
    convert_dir: &std::path::Path,
) {
    if let Some(paths) = crate::infra::dialogs::pick_media_files(convert_dir) {
        controller.add_inputs(paths);
    }
}
