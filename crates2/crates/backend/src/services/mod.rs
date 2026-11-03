pub mod conversion;
pub mod library;
pub mod notices;
pub mod presence;
pub mod previews;
pub mod recording;
pub mod screenshots;
pub mod settings;

pub use conversion::ConversionHandle;
pub use library::LibraryHandle;
pub use notices::NoticeRing;
pub use presence::PresenceHandle;
pub use previews::PreviewHandle;
pub use recording::RecordingHandle;
pub use screenshots::ScreenshotHandle;
pub use settings::SettingsHandle;

#[cfg(test)]
pub(crate) fn test_state() -> mediaa2_protocol::state::State {
    use mediaa2_core::settings::Settings;
    use mediaa2_protocol::state::{ConversionState, RecordingState, RecoveryState, State};

    State {
        settings: Settings::default(),
        recording: RecordingState::Idle,
        conversion: ConversionState::Idle,
        recovery: RecoveryState { running: false },
        notices: Vec::new(),
        picker: None,
        library_revision: 0,
    }
}
