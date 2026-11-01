use crate::logic::recording::RecordTarget;
use crate::logic::recording::RecordingController;
use mediaa_core::limits::DEFAULT_VIDEO_AUDIO_KBPS;
use mediaa_core::settings::{AnimSettings, RecordProfile, RecordSettings, Settings};
use mediaa_core::source::{AudioSource, SourceFactory};
use mediaa_core::{paths, AnimFormat, OutputKind};
use mediaa_media::recorder::{AudioLayout, RecordingRequest};
use mediaa_platform::screen::{self, MonitorInfo, MonitorState, WindowInfo, WindowState};

pub(crate) struct RecordingPlan {
    pub request: RecordingRequest,
    pub audio_only: bool,
    pub allowed_warning: Option<String>,
}

pub(crate) enum PlanError {
    Unavailable(String),
    NoAudioSource,
    Request(String),
}

fn allow_unavailable_sources(profile: RecordProfile, record: &RecordSettings) -> bool {
    profile != RecordProfile::Region && record.allow_unavailable_sources
}

pub(crate) fn target_from_ui(
    settings: &Settings,
    recording: &RecordingController,
    monitors: &[MonitorInfo],
    windows: &[WindowInfo],
) -> Result<RecordTarget, String> {
    use crate::logic::recording::TargetKind;
    match recording.ui.target {
        TargetKind::Screen => {
            if !settings.advanced_settings {
                let monitor = mediaa_platform::cursor::position()
                    .and_then(|(x, y)| screen::monitor_at(x, y))
                    .and_then(|id| monitors.iter().find(|m| m.id == id).cloned())
                    .or_else(|| monitors.first().cloned());
                return monitor
                    .map(|m| RecordTarget::Screens(vec![m]))
                    .ok_or_else(|| rust_i18n::t!("err.no_display").into());
            }
            let mut selected: Vec<_> = monitors
                .iter()
                .filter(|m| recording.ui.monitors.contains(&m.id))
                .cloned()
                .collect();
            if selected.is_empty() {
                selected.extend(monitors.first().cloned());
            }
            if selected.is_empty() {
                Err(rust_i18n::t!("err.no_display").into())
            } else {
                Ok(RecordTarget::Screens(selected))
            }
        }
        TargetKind::Region => recording
            .ui
            .region
            .clone()
            .map(|(m, r)| RecordTarget::Region(m, r))
            .ok_or_else(|| rust_i18n::t!("err.select_area").into()),
        TargetKind::Windows => {
            let selected: Vec<_> = windows
                .iter()
                .filter(|w| recording.ui.windows.contains(&w.id))
                .cloned()
                .collect();
            if selected.is_empty() {
                Err(String::new())
            } else {
                Ok(RecordTarget::Windows(selected))
            }
        }
    }
}

pub(crate) fn unavailable_target(target: &RecordTarget) -> Option<(String, bool)> {
    match target {
        RecordTarget::Audio => None,
        RecordTarget::Screens(monitors) => monitors.iter().find_map(|monitor| {
            (screen::monitor_state(monitor.id) == MonitorState::Disconnected)
                .then(|| (monitor.label(), true))
        }),
        RecordTarget::Region(monitor, _) => (screen::monitor_state(monitor.id)
            == MonitorState::Disconnected)
            .then(|| (monitor.label(), true)),
        RecordTarget::Windows(windows) => windows.iter().find_map(|window| {
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

pub(crate) fn plan(
    settings: &Settings,
    target: &RecordTarget,
    profile: RecordProfile,
    microphones: &[String],
    default_microphone: Option<&str>,
) -> Result<RecordingPlan, PlanError> {
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
        RecordTarget::Screens(monitors) => monitors
            .iter()
            .map(|monitor| {
                let id = monitor.id;
                let size = (monitor.width, monitor.height);
                Box::new(move || {
                    if allow_unavailable
                        && mediaa_platform::screen::monitor_state(id)
                            == mediaa_platform::screen::MonitorState::Disconnected
                    {
                        mediaa_platform::screen::deferred_monitor_source(id, size, None, cursor)
                    } else {
                        screen::monitor_source(id, None, cursor)
                    }
                }) as SourceFactory
            })
            .collect(),
        RecordTarget::Region(monitor, rect) => {
            let (id, rect) = (monitor.id, *rect);
            vec![Box::new(move || {
                if allow_unavailable
                    && mediaa_platform::screen::monitor_state(id)
                        == mediaa_platform::screen::MonitorState::Disconnected
                {
                    mediaa_platform::screen::deferred_monitor_source(
                        id,
                        (rect.w as u32, rect.h as u32),
                        Some(rect),
                        cursor,
                    )
                } else {
                    screen::monitor_source(id, Some(rect), cursor)
                }
            })]
        }
        RecordTarget::Windows(windows) => windows
            .iter()
            .map(|window| {
                let id = window.id;
                let pid = window.pid;
                let app = window.app.clone();
                let size = (window.width, window.height);
                Box::new(move || {
                    if allow_unavailable
                        && mediaa_platform::screen::window_status(id, pid, &app)
                            == mediaa_platform::screen::WindowState::Minimized
                    {
                        mediaa_platform::screen::deferred_window_source(
                            id,
                            size,
                            pid,
                            app.clone(),
                            cursor,
                        )
                    } else {
                        screen::window_source_checked(id, pid, &app, cursor)
                    }
                }) as SourceFactory
            })
            .collect(),
    };

    let audio_only = matches!(target, RecordTarget::Audio);
    let audio_settings = settings.audio_profile(profile);
    if audio_only && audio_settings.is_none() {
        return Err(PlanError::NoAudioSource);
    }
    let audio_settings = audio_settings.unwrap_or(&settings.audio_record);
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
            audio.push(mediaa_platform::audio::input_source(mic));
        }
        if system_audio && mediaa_platform::audio::system_audio_supported() {
            audio.push(mediaa_platform::audio::system_source());
        }
    }

    let now = chrono::Local::now();
    let work_dir = paths::work_dir().join(format!(
        "{}-{}",
        now.format("%Y%m%d-%H%M%S"),
        std::process::id()
    ));
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_platform::screen::{CaptureWindowId, MonitorId, MonitorInfo, WindowInfo};

    #[test]
    fn an_unavailable_window_aborts_the_whole_target() {
        let window = WindowInfo {
            id: CaptureWindowId::new(0),
            title: "Closed window".into(),
            app: String::new(),
            pid: 0,
            width: 800,
            height: 600,
        };
        assert_eq!(
            unavailable_target(&RecordTarget::Windows(vec![window])),
            Some(("Closed window".into(), false))
        );
    }

    #[test]
    fn region_never_allows_unavailable_sources() {
        let mut settings = Settings::default();
        settings.region_record.allow_unavailable_sources = true;
        assert!(!allow_unavailable_sources(
            RecordProfile::Region,
            &settings.region_record
        ));
        settings.record.allow_unavailable_sources = true;
        assert!(allow_unavailable_sources(
            RecordProfile::App,
            &settings.record
        ));
    }

    #[test]
    fn a_disconnected_monitor_can_be_deferred() {
        let monitor = MonitorInfo {
            id: MonitorId::new(u32::MAX),
            device: "gone".into(),
            index: 0,
            name: "Gone monitor".into(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            primary: false,
        };
        assert_eq!(
            unavailable_target(&RecordTarget::Screens(vec![monitor])),
            Some(("Gone monitor (1920×1080)".into(), true))
        );
    }
}
