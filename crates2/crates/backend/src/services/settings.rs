use crate::state::StateStore;
use mediaa2_core::settings::Settings;
use mediaa2_protocol::{CommandError, ErrorCode};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct SettingsHandle {
    state: Arc<StateStore>,
    path: PathBuf,
}

impl SettingsHandle {
    pub fn new(state: Arc<StateStore>, path: PathBuf) -> Self {
        Self { state, path }
    }

    pub async fn apply(&self, settings: Settings) -> Result<Settings, CommandError> {
        let applied = settings.sanitized();
        mediaa2_media::preview_cache::set_animated_previews_enabled(applied.animated_previews);
        mediaa2_media::preview_cache::set_full_previews_enabled(applied.full_preview_videos);
        let saved = applied.clone();
        let path = self.path.clone();
        let result = tokio::task::spawn_blocking(move || saved.save_to(&path)).await;
        match result {
            Ok(Ok(())) => {
                let published = applied.clone();
                self.state
                    .update(|current| current.settings = published.clone());
                Ok(applied)
            }
            Ok(Err(error)) => Err(CommandError {
                code: ErrorCode::Internal,
                message: error.to_string(),
            }),
            Err(error) => Err(CommandError {
                code: ErrorCode::Internal,
                message: error.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::test_state;
    use mediaa2_core::settings::{RecordSettings, ThemePreference};

    #[tokio::test]
    async fn settings_handle_applies_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let state = Arc::new(StateStore::new(test_state()));
        let handle = SettingsHandle::new(Arc::clone(&state), path.clone());
        let next = Settings {
            theme: ThemePreference::Dark,
            record: RecordSettings {
                fps: 30,
                ..RecordSettings::default()
            },
            viewer_volume: 0.5,
            ..Settings::default()
        };

        let applied = handle.apply(next).await.unwrap();

        assert_eq!(applied.theme, ThemePreference::Dark);
        assert_eq!(applied.record.fps, 30);
        assert_eq!(applied.viewer_volume, 0.5);
        assert_eq!(state.snapshot().settings, applied);
        assert_eq!(Settings::load_from(&path), applied);
    }
}
