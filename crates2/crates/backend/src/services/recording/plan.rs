use mediaa2_core::formats::{AnimFormat, OutputKind};
use mediaa2_core::limits::DEFAULT_VIDEO_AUDIO_KBPS;
use mediaa2_core::paths;
use mediaa2_core::recording::RecordTarget;
use mediaa2_core::settings::{AnimSettings, RecordProfile, RecordSettings, Settings};
use mediaa2_core::source::{AudioSource, SourceFactory};
use mediaa2_media::recorder::{AudioLayout, RecordingRequest};
use mediaa2_platform::screen::{self, MonitorState, WindowState};

pub struct RecordingPlan {
    pub request: RecordingRequest,
    pub audio_only: bool,
    pub allowed_warning: Option<String>,
    pub hide: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    Unavailable(String),
    NoAudioSource,
    Request(String),
}

impl PlanError {
    pub fn message(&self) -> String {
        match self {
            PlanError::Unavailable(label) => format!("{label} is not available"),
            PlanError::NoAudioSource => "no sound source is available".into(),
            PlanError::Request(message) => message.clone(),
        }
    }
}

fn allow_unavailable_sources(profile: RecordProfile, record: &RecordSettings) -> bool {
    profile != RecordProfile::Region && record.allow_unavailable_sources
}

pub fn unavailable_target(target: &RecordTarget) -> Option<(String, bool)> {
    match target {
        RecordTarget::Audio => None,
        RecordTarget::Screens { monitors } => monitors.iter().find_map(|monitor| {
            (screen::monitor_state(monitor.id) == MonitorState::Disconnected)
                .then(|| (monitor.label(), true))
        }),
        RecordTarget::Region { monitor, .. } => (screen::monitor_state(monitor.id)
            == MonitorState::Disconnected)
            .then(|| (monitor.label(), true)),
        RecordTarget::Windows { windows } => windows.iter().find_map(|window| {
            match screen::window_status(window.id, window.pid, &window.app) {
                WindowState::Available => None,
                WindowState::Minimized => Some((window.label(), true)),
                WindowState::Hidden | WindowState::Missing | WindowState::Mismatch => {
                    Some((window.label(), false))
                }
            }
        }),
    }
}

fn needs_a_sound_source(
    settings: &Settings,
    target: &RecordTarget,
    profile: RecordProfile,
) -> bool {
    if matches!(target, RecordTarget::Audio) {
        return settings.audio_profile(profile).is_some();
    }
    let (record, _) = settings.profile(profile);
    record.records_audio()
}

fn precheck(
    settings: &Settings,
    target: &RecordTarget,
    profile: RecordProfile,
) -> Result<(), PlanError> {
    if matches!(target, RecordTarget::Audio) && settings.audio_profile(profile).is_none() {
        return Err(PlanError::NoAudioSource);
    }
    let (record, _) = settings.profile(profile);
    if let Some((label, allowed)) = unavailable_target(target) {
        if !allowed || !allow_unavailable_sources(profile, record) {
            return Err(PlanError::Unavailable(label));
        }
    }
    Ok(())
}

pub fn plan(
    settings: &Settings,
    target: &RecordTarget,
    profile: RecordProfile,
    hide: bool,
) -> Result<RecordingPlan, PlanError> {
    precheck(settings, target, profile)?;
    if !needs_a_sound_source(settings, target, profile) {
        return plan_with_devices(settings, target, profile, hide, &[], None);
    }
    let microphones = mediaa2_platform::audio::input_devices();
    let default = mediaa2_platform::audio::default_input_device();
    plan_with_devices(
        settings,
        target,
        profile,
        hide,
        &microphones,
        default.as_deref(),
    )
}

pub fn plan_with_devices(
    settings: &Settings,
    target: &RecordTarget,
    profile: RecordProfile,
    hide: bool,
    microphones: &[String],
    default_microphone: Option<&str>,
) -> Result<RecordingPlan, PlanError> {
    precheck(settings, target, profile)?;
    let (record, quality) = settings.profile(profile);
    let allow_unavailable = allow_unavailable_sources(profile, record);
    let mut allowed_warning = None;
    if let Some((label, allowed)) = unavailable_target(target) {
        if !allowed || !allow_unavailable {
            return Err(PlanError::Unavailable(label));
        }
        allowed_warning = Some(label);
    }

    let webp = matches!(record.output, OutputKind::Anim(AnimFormat::Webp));
    let (fps, max_height) = if webp {
        (record.animation_fps(), record.animation_height())
    } else {
        (record.fps, record.max_height())
    };
    let animation = AnimSettings::for_recording(fps, max_height, quality);
    let cursor = record.record_cursor;
    let sources: Vec<SourceFactory> = match target {
        RecordTarget::Audio => Vec::new(),
        RecordTarget::Screens { monitors } => monitors
            .iter()
            .map(|monitor| {
                let id = monitor.id;
                let size = (monitor.width, monitor.height);
                Box::new(move || {
                    if allow_unavailable && screen::monitor_state(id) == MonitorState::Disconnected
                    {
                        screen::deferred_monitor_source(id, size, None, cursor)
                    } else {
                        screen::monitor_source(id, None, cursor)
                    }
                }) as SourceFactory
            })
            .collect(),
        RecordTarget::Region { monitor, region } => {
            let (id, region) = (monitor.id, *region);
            vec![Box::new(move || {
                if allow_unavailable && screen::monitor_state(id) == MonitorState::Disconnected {
                    screen::deferred_monitor_source(
                        id,
                        (region.w as u32, region.h as u32),
                        Some(region),
                        cursor,
                    )
                } else {
                    screen::monitor_source(id, Some(region), cursor)
                }
            })]
        }
        RecordTarget::Windows { windows } => windows
            .iter()
            .map(|window| {
                let id = window.id;
                let pid = window.pid;
                let app = window.app.clone();
                let size = (window.width, window.height);
                Box::new(move || {
                    if allow_unavailable
                        && screen::window_status(id, pid, &app) == WindowState::Minimized
                    {
                        screen::deferred_window_source(id, size, pid, app.clone(), cursor)
                    } else {
                        screen::window_source_checked(id, pid, &app, cursor)
                    }
                }) as SourceFactory
            })
            .collect(),
    };

    let audio_only = matches!(target, RecordTarget::Audio);
    if audio_only && settings.audio_profile(profile).is_none() {
        return Err(PlanError::NoAudioSource);
    }
    let audio_settings = settings
        .audio_profile(profile)
        .unwrap_or(&settings.audio_record);
    let (want_sound, microphone, system_audio) = if audio_only {
        (
            true,
            &audio_settings.microphone,
            audio_settings.system_audio,
        )
    } else {
        (
            record.records_audio(),
            &record.microphone,
            record.system_audio,
        )
    };
    let mut audio: Vec<Box<dyn AudioSource>> = Vec::new();
    if want_sound {
        if let Some(mic) = microphone.resolve(microphones, default_microphone) {
            log::info!("microphone: {mic}");
            audio.push(mediaa2_platform::audio::input_source(mic));
        }
        if system_audio && mediaa2_platform::audio::system_audio_supported() {
            audio.push(mediaa2_platform::audio::system_source());
        }
    }

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let work_dir = paths::work_dir().join(format!("{stamp}-{}", std::process::id()));
    let output = if audio_only {
        OutputKind::Audio(audio_settings.format)
    } else {
        record.output
    };
    let request = RecordingRequest::builder(
        output,
        sources,
        audio,
        paths::MediaDir::new(settings.media_dir.clone()),
        paths::WorkDir::new(work_dir),
        if audio_only { "Audio" } else { "Recording" }.into(),
    )
    .video(fps, record.video_quality, max_height)
    .audio_layout(AudioLayout::EveryFile)
    .audio_kbps(if audio_only {
        audio_settings.kbps
    } else {
        DEFAULT_VIDEO_AUDIO_KBPS
    })
    .animation(animation)
    .webp_mode(record.webp_mode())
    .no_signal(record.show_no_signal)
    .build()
    .map_err(|error| PlanError::Request(format!("{error}")))?;

    Ok(RecordingPlan {
        request,
        audio_only,
        allowed_warning,
        hide,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa2_core::formats::{AudioFormat, VideoContainer};
    use mediaa2_core::geometry::Rect;
    use mediaa2_core::screen::{CaptureWindowId, MonitorId, MonitorInfo, WindowInfo};
    use mediaa2_core::settings::MicChoice;

    fn monitor(id: u32) -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::new(id),
            device: format!(r"\\.\DISPLAY{id}"),
            index: 0,
            name: format!("Display {id}"),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            primary: false,
        }
    }

    fn window() -> WindowInfo {
        WindowInfo {
            id: CaptureWindowId::new(0),
            title: "Closed window".into(),
            app: String::new(),
            pid: 0,
            width: 800,
            height: 600,
        }
    }

    fn quiet_settings() -> Settings {
        let mut settings = Settings::default();
        settings.record.record_audio = false;
        settings.record.system_audio = false;
        settings.hotkey_record.record_audio = false;
        settings.hotkey_record.system_audio = false;
        settings.region_record.record_audio = false;
        settings.region_record.system_audio = false;
        settings.audio_record.microphone = MicChoice::Off;
        settings.audio_record.system_audio = false;
        settings
    }

    #[test]
    fn a_missing_window_aborts_the_whole_target() {
        let settings = quiet_settings();
        let target = RecordTarget::Windows {
            windows: vec![window()],
        };
        match plan_with_devices(&settings, &target, RecordProfile::App, false, &[], None) {
            Err(PlanError::Unavailable(label)) => assert_eq!(label, "Closed window"),
            Err(other) => panic!("unexpected plan error: {other:?}"),
            Ok(_) => panic!("a closed window must not be planned"),
        }
    }

    #[test]
    fn a_disconnected_monitor_can_be_deferred_for_the_app_profile() {
        let mut settings = quiet_settings();
        settings.record.allow_unavailable_sources = true;
        let target = RecordTarget::Screens {
            monitors: vec![monitor(u32::MAX)],
        };
        let plan = plan_with_devices(&settings, &target, RecordProfile::App, true, &[], None)
            .expect("a deferred monitor is allowed");
        assert_eq!(
            plan.allowed_warning.as_deref(),
            Some("Display 4294967295 (1920×1080)")
        );
        assert!(plan.hide);
        assert!(!plan.audio_only);
        assert_eq!(plan.request.output(), settings.record.output);
    }

    #[test]
    fn a_region_never_allows_an_unavailable_monitor() {
        let mut settings = quiet_settings();
        settings.region_record.allow_unavailable_sources = true;
        let target = RecordTarget::Region {
            monitor: monitor(u32::MAX),
            region: Rect::new(0, 0, 320, 200),
        };
        assert!(matches!(
            plan_with_devices(&settings, &target, RecordProfile::Region, false, &[], None),
            Err(PlanError::Unavailable(_))
        ));
    }

    #[test]
    fn audio_only_needs_an_audio_profile() {
        let settings = quiet_settings();
        assert!(matches!(
            plan_with_devices(
                &settings,
                &RecordTarget::Audio,
                RecordProfile::Region,
                false,
                &[],
                None
            ),
            Err(PlanError::NoAudioSource)
        ));
    }

    #[test]
    fn audio_only_without_any_source_is_a_request_error() {
        let settings = quiet_settings();
        assert!(matches!(
            plan_with_devices(
                &settings,
                &RecordTarget::Audio,
                RecordProfile::App,
                false,
                &[],
                None
            ),
            Err(PlanError::Request(_))
        ));
    }

    #[test]
    fn audio_only_uses_the_profile_audio_format() {
        let mut settings = quiet_settings();
        settings.audio_record.microphone = MicChoice::Device("USB Mic".into());
        settings.hotkey_audio.format = AudioFormat::Flac;
        settings.hotkey_audio.microphone = MicChoice::Device("USB Mic".into());
        let plan = plan_with_devices(
            &settings,
            &RecordTarget::Audio,
            RecordProfile::Hotkey,
            false,
            &["USB Mic".to_string()],
            None,
        )
        .expect("an audio-only profile with a device plans");
        assert!(plan.audio_only);
        assert_eq!(plan.request.output(), OutputKind::Audio(AudioFormat::Flac));
        assert_eq!(
            plan_with_devices(
                &settings,
                &RecordTarget::Audio,
                RecordProfile::App,
                false,
                &["USB Mic".to_string()],
                None,
            )
            .expect("the app audio profile plans")
            .request
            .output(),
            OutputKind::Audio(AudioFormat::Mp3)
        );
    }

    #[test]
    fn screen_profiles_use_their_own_output() {
        let mut settings = quiet_settings();
        settings.hotkey_record.output = OutputKind::Video(VideoContainer::Mp4);
        settings.hotkey_record.allow_unavailable_sources = true;
        let target = RecordTarget::Screens {
            monitors: vec![monitor(u32::MAX)],
        };
        let plan = plan_with_devices(&settings, &target, RecordProfile::Hotkey, false, &[], None)
            .expect("the hotkey profile allows a deferred monitor");
        assert_eq!(
            plan.request.output(),
            OutputKind::Video(VideoContainer::Mp4)
        );
    }
}
