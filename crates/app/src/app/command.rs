use crate::features::recording::{RecordTarget, StopSource};
use mediaa_core::settings::RecordProfile;
use mediaa_media::recorder::StopAction;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RecordingCommand {
    Start {
        target: RecordTarget,
        profile: RecordProfile,
    },
    Stop {
        action: StopAction,
        source: StopSource,
    },
    TogglePause,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AppCommand {
    Recording(RecordingCommand),
    Quit,
}
