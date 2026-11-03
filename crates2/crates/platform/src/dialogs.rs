use mediaa2_core::formats;
use std::path::{Path, PathBuf};

pub fn pick_folder(start: &Path) -> Option<PathBuf> {
    rfd::FileDialog::new().set_directory(start).pick_folder()
}

pub fn pick_convert_files(directory: &Path) -> Option<Vec<PathBuf>> {
    let mut dialog = rfd::FileDialog::new()
        .add_filter("Media", &formats::all_media_extensions())
        .add_filter("All files", &["*"]);
    if directory.is_dir() {
        dialog = dialog.set_directory(directory);
    }
    dialog.pick_files()
}

pub fn show_error(title: &str, detail: &str) {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(title)
        .set_description(detail)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}
