use std::path::{Path, PathBuf};

pub const TEMP_PREFIX: &str = ".mediaa-";

pub struct AtomicFile {
    target: PathBuf,
    temp: tempfile::TempPath,
}

impl AtomicFile {
    pub fn new(target: &Path) -> anyhow::Result<Self> {
        let parent = target.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let extension = target
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| format!(".{value}"))
            .unwrap_or_default();
        let temp = tempfile::Builder::new()
            .prefix(TEMP_PREFIX)
            .suffix(&extension)
            .tempfile_in(parent)?
            .into_temp_path();
        std::fs::remove_file(&temp)?;
        Ok(Self {
            target: target.to_path_buf(),
            temp,
        })
    }

    pub fn path(&self) -> &Path {
        &self.temp
    }

    pub fn commit(self) -> anyhow::Result<PathBuf> {
        self.prepare()?;
        self.temp
            .persist_noclobber(&self.target)
            .map_err(|error| error.error)?;
        Ok(self.target)
    }

    pub fn commit_replace(self) -> anyhow::Result<PathBuf> {
        self.prepare()?;
        self.temp
            .persist(&self.target)
            .map_err(|error| error.error)?;
        Ok(self.target)
    }

    fn prepare(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.temp.is_file(),
            "encoder did not create {}",
            self.temp.display()
        );
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.temp)?
            .sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::AtomicFile;

    #[test]
    fn commit_moves_the_finished_file_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("result.mp4");
        let output = AtomicFile::new(&target).unwrap();
        std::fs::write(output.path(), b"ready").unwrap();
        assert_eq!(output.commit().unwrap(), target);
        assert_eq!(std::fs::read(&target).unwrap(), b"ready");

        std::fs::write(&target, b"existing").unwrap();
        let output = AtomicFile::new(&target).unwrap();
        std::fs::write(output.path(), b"replacement").unwrap();
        assert!(output.commit().is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"existing");
    }

    #[test]
    fn commit_replace_publishes_new_contents_over_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("settings.toml");
        std::fs::write(&target, b"old").unwrap();
        let output = AtomicFile::new(&target).unwrap();
        std::fs::write(output.path(), b"new").unwrap();
        output.commit_replace().unwrap();
        assert_eq!(std::fs::read(target).unwrap(), b"new");
    }
}
