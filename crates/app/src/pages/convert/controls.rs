use super::*;
use mediaa_core::limits::VIDEO_QUALITY;

pub(super) fn estimate_row(
    ui: &mut egui::Ui,
    estimate: Option<&Estimate>,
    input: Option<&std::path::Path>,
) {
    let Some(e) = estimate else { return };
    let source = input
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len());
    let text = |bytes: u64| {
        let mut s = format!("≈ {}", human_size(bytes));
        if let Some(src) = source.filter(|s| *s > 0) {
            let pct = (bytes as f64 / src as f64 - 1.0) * 100.0;
            s += &format!(
                "  ({}{:.0} %)",
                if pct < 0.0 { "−" } else { "+" },
                pct.abs()
            );
        }
        s
    };
    theme::row(ui, &t!("convert.size"), |ui| {
        let resp = match &e.result {
            Some(Ok(bytes)) => ui.label(RichText::new(text(*bytes)).strong()),
            Some(Err(err)) => ui
                .label(RichText::new(t!("convert.size_failed")).color(theme::palette(ui).text_weak))
                .on_hover_text(err),
            None => {
                ui.add(egui::Spinner::new().size(12.0));
                match e.previous {
                    Some(bytes) => {
                        ui.label(RichText::new(text(bytes)).color(theme::palette(ui).text_weak))
                    }
                    None => ui.label(
                        RichText::new(t!("convert.size_estimating"))
                            .color(theme::palette(ui).text_weak),
                    ),
                }
            }
        };
        resp.on_hover_text(t!("convert.size_hint"));
    });
}

pub(super) fn effort_row(ui: &mut egui::Ui, effort: &mut Effort) {
    theme::row(ui, &t!("convert.effort"), |ui| {
        let opts: Vec<(Effort, String)> = Effort::ALL
            .iter()
            .map(|e| {
                let label = match e {
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
    egui::ComboBox::from_id_salt("convert-kbps")
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
    egui::ComboBox::from_id_salt("convert-fps")
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
    egui::ComboBox::from_id_salt("convert-colors")
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

pub(super) fn pick_file(app: &mut App) {
    let mut dialog = rfd::FileDialog::new()
        .add_filter(
            t!("convert.filter_media"),
            &mediaa_core::formats::all_media_extensions(),
        )
        .add_filter(t!("convert.filter_all"), &["*"]);
    if app.settings.media_dir.is_dir() {
        dialog = dialog.set_directory(&app.settings.media_dir);
    }
    if let Some(path) = dialog.pick_file() {
        app.conv.set_input(path);
    }
}
