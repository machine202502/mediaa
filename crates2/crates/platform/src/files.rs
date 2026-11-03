use anyhow::Context;
use std::path::Path;

pub fn open(path: &Path) -> anyhow::Result<()> {
    opener::open(path).with_context(|| format!("cannot open {}", path.display()))
}

pub fn reveal(path: &Path) -> anyhow::Result<()> {
    opener::reveal(path).with_context(|| format!("cannot show {}", path.display()))
}

pub fn move_to_trash(path: &Path) -> anyhow::Result<()> {
    trash::delete(path).with_context(|| format!("cannot delete {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_missing_file_fails_cleanly() {
        assert!(move_to_trash(Path::new("/definitely/not/here.png")).is_err());
    }
}
