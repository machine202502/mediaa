use mediaa_core::ImageFormat;
use std::path::{Path, PathBuf};

pub(crate) fn pick_save_path(
    directory: &Path,
    stem: String,
    formats: [ImageFormat; 2],
) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .set_directory(directory)
        .set_file_name(stem);
    for format in formats {
        dialog = dialog.add_filter(format.label(), &[format.extension()]);
    }
    dialog.save_file()
}

pub(crate) fn pick_folder(start: &Path) -> Option<PathBuf> {
    rfd::FileDialog::new().set_directory(start).pick_folder()
}

pub(crate) fn pick_media_files(directory: &Path) -> Option<Vec<PathBuf>> {
    let mut dialog = rfd::FileDialog::new()
        .add_filter(
            rust_i18n::t!("convert.filter_media"),
            &mediaa_core::formats::all_media_extensions(),
        )
        .add_filter(rust_i18n::t!("convert.filter_all"), &["*"]);
    if directory.is_dir() {
        dialog = dialog.set_directory(directory);
    }
    dialog.pick_files()
}

pub(crate) fn pick_catalog_files() -> Option<Vec<PathBuf>> {
    rfd::FileDialog::new()
        .add_filter(
            rust_i18n::t!("agent.catalog_files"),
            &mediaa_core::catalogs::accepted_extensions(),
        )
        .pick_files()
}
