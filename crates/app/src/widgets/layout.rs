use eframe::egui;

pub fn centered_widget(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    let offset = ((ui.available_width() - width) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(offset);
        add(ui);
    });
}

pub fn centered_row(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.vertical_centered(|ui| {
        egui::Grid::new(ui.next_auto_id())
            .num_columns(8)
            .spacing(egui::vec2(ui.spacing().item_spacing.x, 0.0))
            .show(ui, |ui| add(ui));
    });
}

pub fn centered_row_sized(ui: &mut egui::Ui, content_width: f32, add: impl FnOnce(&mut egui::Ui)) {
    let offset = ((ui.available_width() - content_width) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(offset);
        add(ui);
    });
}
