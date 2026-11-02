use std::path::Path;

pub(crate) fn reveal(path: &Path) -> anyhow::Result<()> {
    mediaa_platform::files::reveal(path)
}
