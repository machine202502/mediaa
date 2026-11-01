use std::path::Path;

pub(crate) fn app_path(executable: &Path) -> String {
    format!("\"{}\"", executable.display())
}
