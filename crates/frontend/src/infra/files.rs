use std::path::Path;

pub(crate) fn ensure_dir(path: &Path) -> bool {
    std::fs::create_dir_all(path).is_ok()
}
