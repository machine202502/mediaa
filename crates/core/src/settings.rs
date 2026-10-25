use crate::formats::{AnimFormat, AudioFormat, ImageFormat, OutputKind, VideoContainer};
use crate::limits::{AUDIO_KBPS, DEFAULT_MUSIC_AUDIO_KBPS, FPS, GIF_COLORS, VIDEO_QUALITY};
use crate::paths;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AfterCapture {
    Copy,
    Save,
    #[default]
    CopyAndSave,
}

impl AfterCapture {
    pub const ALL: [AfterCapture; 3] = [
        AfterCapture::Copy,
        AfterCapture::Save,
        AfterCapture::CopyAndSave,
    ];

    pub fn copies(self) -> bool {
        matches!(self, AfterCapture::Copy | AfterCapture::CopyAndSave)
    }

    pub fn saves(self) -> bool {
        matches!(self, AfterCapture::Save | AfterCapture::CopyAndSave)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GifDither {
    None,
    Bayer,
    #[default]
    Sierra2,
    FloydSteinberg,
}

impl GifDither {
    pub const ALL: [GifDither; 4] = [
        GifDither::None,
        GifDither::Bayer,
        GifDither::Sierra2,
        GifDither::FloydSteinberg,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GifDither::None => "None",
            GifDither::Bayer => "Bayer",
            GifDither::Sierra2 => "Sierra-2",
            GifDither::FloydSteinberg => "Floyd–Steinberg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenshotSettings {
    pub format: ImageFormat,
    pub after_capture: AfterCapture,
    pub hide_window: bool,
}

impl Default for ScreenshotSettings {
    fn default() -> Self {
        Self {
            format: ImageFormat::Png,
            after_capture: AfterCapture::CopyAndSave,
            hide_window: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MicChoice {
    #[default]
    Auto,
    Off,
    Device(String),
}

impl MicChoice {
    pub fn resolve(&self, devices: &[String], default: Option<&str>) -> Option<String> {
        let fallback = || {
            default
                .map(str::to_string)
                .or_else(|| devices.first().cloned())
        };
        match self {
            MicChoice::Off => None,
            MicChoice::Auto => fallback(),
            MicChoice::Device(name) if devices.contains(name) => Some(name.clone()),
            MicChoice::Device(_) => fallback(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordSettings {
    pub output: OutputKind,
    pub record_audio: bool,
    pub fps: u32,
    pub video_quality: u8,
    pub microphone: MicChoice,
    pub system_audio: bool,
    pub audio_in_every_file: bool,
    pub record_cursor: bool,
    pub output_height: u32,
    pub hide_to_tray: bool,
    pub segmented_webp: bool,
    pub parallel_webp: bool,
}

impl Default for RecordSettings {
    fn default() -> Self {
        Self {
            output: OutputKind::default(),
            record_audio: true,
            fps: 60,
            video_quality: 100,
            microphone: MicChoice::Auto,
            system_audio: true,
            audio_in_every_file: true,
            record_cursor: true,
            output_height: 2160,
            hide_to_tray: true,
            segmented_webp: true,
            parallel_webp: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioRecordSettings {
    pub format: AudioFormat,
    pub kbps: u32,
    pub microphone: MicChoice,
    pub system_audio: bool,
}

impl Default for AudioRecordSettings {
    fn default() -> Self {
        Self {
            format: AudioFormat::Mp3,
            kbps: DEFAULT_MUSIC_AUDIO_KBPS,
            microphone: MicChoice::Auto,
            system_audio: true,
        }
    }
}

pub const AUDIO_BITRATES: [u32; 5] = [96, 128, 192, 256, 320];

pub const OUTPUT_HEIGHTS: [u32; 5] = [2160, 1440, 1080, 720, 480];

pub const ANIM_HEIGHTS: [u32; 5] = [1080, 720, 480, 360, 240];

impl RecordSettings {
    pub fn max_height(&self) -> Option<u32> {
        (self.output_height > 0).then_some(self.output_height)
    }

    pub fn records_audio(&self) -> bool {
        self.output.supports_audio()
            && self.record_audio
            && (self.microphone != MicChoice::Off || self.system_audio)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimSettings {
    pub fps: u32,
    pub max_height: u32,
    pub gif_colors: u32,
    pub gif_dither: GifDither,
    pub webp_quality: u8,
}

impl Default for AnimSettings {
    fn default() -> Self {
        Self {
            fps: 15,
            max_height: 480,
            gif_colors: 256,
            gif_dither: GifDither::Sierra2,
            webp_quality: 80,
        }
    }
}

impl AnimSettings {
    pub fn height_limit(&self) -> Option<u32> {
        (self.max_height > 0).then_some(self.max_height)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeySettings {
    pub enabled: bool,
    pub region_screenshot: String,
    pub fullscreen_screenshot: String,
    pub start_video: String,
    pub start_audio: String,
    pub stop_recording: String,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            region_screenshot: String::new(),
            fullscreen_screenshot: String::new(),
            start_video: String::new(),
            start_audio: String::new(),
            stop_recording: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrayClickAction {
    OpenWindow,
    #[default]
    ScreenArea,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AfterCompletionAction {
    #[default]
    OpenLibrary,
    CopyFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScreenChoice {
    #[default]
    Primary,
    UnderCursor,
    Display(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordProfile {
    App,
    Hotkey,
    Region,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemePreference,
    pub language: Option<String>,
    pub media_dir: PathBuf,
    pub convert_dir: Option<PathBuf>,
    pub screenshot: ScreenshotSettings,
    pub record: RecordSettings,
    pub anim: AnimSettings,
    pub hotkey_record: RecordSettings,
    pub hotkey_anim: AnimSettings,
    pub region_record: RecordSettings,
    pub region_anim: AnimSettings,
    pub hotkey_screen: ScreenChoice,
    pub audio_record: AudioRecordSettings,
    pub hotkey_audio: AudioRecordSettings,
    pub hotkeys: HotkeySettings,
    pub launch_at_login: bool,
    pub start_in_tray: bool,
    pub close_to_tray: bool,
    pub tray_left_click: TrayClickAction,
    pub stop_recording_on_left_click: bool,
    pub after_completion: AfterCompletionAction,
    pub highlight_recording_region: bool,
    pub recording_region_outline_width: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            language: None,
            media_dir: paths::default_media_dir(),
            convert_dir: None,
            screenshot: ScreenshotSettings::default(),
            record: RecordSettings::default(),
            anim: AnimSettings::default(),
            hotkey_record: RecordSettings {
                output: OutputKind::Video(VideoContainer::Mkv),
                ..RecordSettings::default()
            },
            hotkey_anim: AnimSettings::default(),
            region_record: RecordSettings {
                output: OutputKind::Anim(AnimFormat::Webp),
                fps: AnimSettings::default().fps,
                output_height: AnimSettings::default().max_height,
                ..RecordSettings::default()
            },
            region_anim: AnimSettings::default(),
            hotkey_screen: ScreenChoice::Primary,
            audio_record: AudioRecordSettings::default(),
            hotkey_audio: AudioRecordSettings::default(),
            hotkeys: HotkeySettings::default(),
            launch_at_login: false,
            start_in_tray: false,
            close_to_tray: true,
            tray_left_click: TrayClickAction::default(),
            stop_recording_on_left_click: true,
            after_completion: AfterCompletionAction::default(),
            highlight_recording_region: true,
            recording_region_outline_width: 4,
        }
    }
}

impl Settings {
    pub fn load_from(path: &Path) -> Settings {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str::<Settings>(&text) {
                Ok(s) => s.sanitized(),
                Err(e) => {
                    log::warn!(
                        "settings: {} is invalid ({e}); using defaults",
                        path.display()
                    );
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn load() -> Settings {
        Self::load_from(&paths::settings_file())
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&paths::settings_file())
    }

    pub fn sanitized(mut self) -> Settings {
        for r in [
            &mut self.record,
            &mut self.hotkey_record,
            &mut self.region_record,
        ] {
            r.fps = FPS.clamp(r.fps);
            r.video_quality = VIDEO_QUALITY.clamp(u32::from(r.video_quality)) as u8;
        }
        for a in [&mut self.anim, &mut self.hotkey_anim, &mut self.region_anim] {
            a.fps = FPS.clamp(a.fps);
            a.gif_colors = GIF_COLORS.clamp(a.gif_colors);
            a.webp_quality = VIDEO_QUALITY.clamp(u32::from(a.webp_quality)) as u8;
        }
        for audio in [&mut self.audio_record, &mut self.hotkey_audio] {
            audio.kbps = AUDIO_KBPS.clamp(audio.kbps);
        }
        if !matches!(self.hotkey_record.output, OutputKind::Video(_)) {
            self.hotkey_record.output = OutputKind::Video(VideoContainer::Mkv);
        }
        if self.media_dir.as_os_str().is_empty() {
            self.media_dir = paths::default_media_dir();
        }
        self.recording_region_outline_width = self.recording_region_outline_width.clamp(2, 32);
        self
    }

    pub fn convert_dir(&self) -> PathBuf {
        self.convert_dir
            .clone()
            .unwrap_or_else(|| self.media_dir.clone())
    }

    pub fn audio_profile(&self, p: RecordProfile) -> Option<&AudioRecordSettings> {
        match p {
            RecordProfile::App => Some(&self.audio_record),
            RecordProfile::Hotkey => Some(&self.hotkey_audio),
            RecordProfile::Region => None,
        }
    }

    pub fn profile(&self, p: RecordProfile) -> (&RecordSettings, &AnimSettings) {
        match p {
            RecordProfile::App => (&self.record, &self.anim),
            RecordProfile::Hotkey => (&self.hotkey_record, &self.hotkey_anim),
            RecordProfile::Region => (&self.region_record, &self.region_anim),
        }
    }

    pub fn profile_mut(&mut self, p: RecordProfile) -> (&mut RecordSettings, &mut AnimSettings) {
        match p {
            RecordProfile::App => (&mut self.record, &mut self.anim),
            RecordProfile::Hotkey => (&mut self.hotkey_record, &mut self.hotkey_anim),
            RecordProfile::Region => (&mut self.region_record, &mut self.region_anim),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::{AnimFormat, VideoContainer};

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn roundtrip_through_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.toml");
        let mut s = Settings::default();
        s.theme = ThemePreference::Dark;
        s.language = Some("ru".into());
        s.record.output = OutputKind::Anim(AnimFormat::Webp);
        s.record.microphone = MicChoice::Device("USB Mic".into());
        s.anim.max_height = 360;
        s.tray_left_click = TrayClickAction::OpenWindow;
        s.stop_recording_on_left_click = false;
        s.after_completion = AfterCompletionAction::CopyFile;
        s.highlight_recording_region = false;
        s.recording_region_outline_width = 17;
        s.record.segmented_webp = false;
        s.record.parallel_webp = false;
        s.launch_at_login = true;
        s.start_in_tray = true;
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let s = Settings::load_from(Path::new("/nope/settings.toml"));
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn partial_file_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.toml");
        std::fs::write(&path, "theme =\"light\"\n[record]\nfps = 24\n").unwrap();
        let s = Settings::load_from(&path);
        assert_eq!(s.theme, ThemePreference::Light);
        assert_eq!(s.record.fps, 24);
        assert_eq!(
            s.record.video_quality,
            RecordSettings::default().video_quality
        );
        assert_eq!(s.anim, AnimSettings::default());
    }

    #[test]
    fn invalid_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.toml");
        std::fs::write(&path, "this is = = not toml").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
    }

    #[test]
    fn closing_the_window_hides_to_the_tray_by_default() {
        assert!(Settings::default().close_to_tray);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.toml");
        std::fs::write(&path, "theme = \"light\"\n").unwrap();
        assert!(
            Settings::load_from(&path).close_to_tray,
            "an old file without the key keeps the default"
        );
        std::fs::write(&path, "close_to_tray = false\n").unwrap();
        assert!(!Settings::load_from(&path).close_to_tray);
    }

    #[test]
    fn autostart_and_tray_start_are_disabled_by_default() {
        let settings = Settings::default();
        assert!(!settings.launch_at_login);
        assert!(!settings.start_in_tray);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.toml");
        std::fs::write(&path, "close_to_tray = true\n").unwrap();
        let loaded = Settings::load_from(&path);
        assert!(!loaded.launch_at_login);
        assert!(!loaded.start_in_tray);
        std::fs::write(&path, "launch_at_login = true\nstart_in_tray = true\n").unwrap();
        let loaded = Settings::load_from(&path);
        assert!(loaded.launch_at_login);
        assert!(loaded.start_in_tray);
    }

    #[test]
    fn tray_left_click_starts_a_screen_area_by_default() {
        assert_eq!(TrayClickAction::default(), TrayClickAction::ScreenArea);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.toml");
        std::fs::write(&path, "close_to_tray = true\n").unwrap();
        assert_eq!(
            Settings::load_from(&path).tray_left_click,
            TrayClickAction::ScreenArea
        );
        std::fs::write(&path, "tray_left_click = \"open_window\"\n").unwrap();
        assert_eq!(
            Settings::load_from(&path).tray_left_click,
            TrayClickAction::OpenWindow
        );
    }

    #[test]
    fn tray_left_click_stops_recording_by_default_and_can_be_disabled() {
        assert!(Settings::default().stop_recording_on_left_click);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.toml");
        std::fs::write(&path, "tray_left_click = \"open_window\"\n").unwrap();
        assert!(Settings::load_from(&path).stop_recording_on_left_click);
        std::fs::write(&path, "stop_recording_on_left_click = false\n").unwrap();
        assert!(!Settings::load_from(&path).stop_recording_on_left_click);
    }

    #[test]
    fn completion_opens_the_library_by_default() {
        assert_eq!(
            AfterCompletionAction::default(),
            AfterCompletionAction::OpenLibrary
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.toml");
        std::fs::write(&path, "close_to_tray = true\n").unwrap();
        assert_eq!(
            Settings::load_from(&path).after_completion,
            AfterCompletionAction::OpenLibrary
        );
        std::fs::write(&path, "after_completion = \"copy_file\"\n").unwrap();
        assert_eq!(
            Settings::load_from(&path).after_completion,
            AfterCompletionAction::CopyFile
        );
    }

    #[test]
    fn recording_region_highlight_is_enabled_by_default() {
        assert!(Settings::default().highlight_recording_region);
        assert_eq!(Settings::default().recording_region_outline_width, 4);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.toml");
        std::fs::write(&path, "theme = \"light\"\n").unwrap();
        assert!(Settings::load_from(&path).highlight_recording_region);
        std::fs::write(&path, "highlight_recording_region = false\n").unwrap();
        assert!(!Settings::load_from(&path).highlight_recording_region);
    }

    #[test]
    fn values_are_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.toml");
        std::fs::write(&path, "[record]\nfps = 1000\nvideo_quality = 0\n[hotkey_record]\nfps = 0\n[anim]\ngif_colors = 9999\n").unwrap();
        let s = Settings::load_from(&path);
        assert_eq!(s.record.fps, 60);
        assert_eq!(s.record.video_quality, 1);
        assert_eq!(s.hotkey_record.fps, 1);
        assert_eq!(s.anim.gif_colors, 256);
    }

    #[test]
    fn audio_only_with_video_output_and_source() {
        let mut r = RecordSettings {
            system_audio: false,
            microphone: MicChoice::Off,
            ..RecordSettings::default()
        };
        assert!(!r.records_audio());
        r.system_audio = true;
        assert!(r.records_audio());
        r.output = OutputKind::Anim(AnimFormat::Gif);
        assert!(!r.records_audio());
        r.output = OutputKind::Video(VideoContainer::Mkv);
        assert!(r.records_audio());
        r.record_audio = false;
        assert!(!r.records_audio(), "master switch off");
    }

    #[test]
    fn microphone_resolution() {
        let devs = vec!["Headset".to_string(), "Webcam mic".to_string()];
        assert_eq!(
            MicChoice::Auto.resolve(&devs, Some("Webcam mic")),
            Some("Webcam mic".into())
        );
        assert_eq!(
            MicChoice::Auto.resolve(&devs, None),
            Some("Headset".into()),
            "no default: first device"
        );
        assert_eq!(
            MicChoice::Auto.resolve(&[], None),
            None,
            "nothing connected"
        );
        assert_eq!(
            MicChoice::Off.resolve(&devs, Some("Headset")),
            None,
            "explicit None stays off"
        );
        let headset = MicChoice::Device("Headset".into());
        assert_eq!(
            headset.resolve(&devs, Some("Webcam mic")),
            Some("Headset".into())
        );
        let gone = MicChoice::Device("USB Mic".into());
        assert_eq!(
            gone.resolve(&devs, Some("Webcam mic")),
            Some("Webcam mic".into()),
            "unplugged: default instead"
        );
    }

    #[test]
    fn recording_defaults() {
        let r = RecordSettings::default();
        assert_eq!(r.output, OutputKind::Video(VideoContainer::Mp4));
        assert!(
            r.record_audio
                && r.system_audio
                && r.microphone == MicChoice::Auto
                && r.audio_in_every_file
                && r.record_cursor
        );
        assert!(r.segmented_webp && r.parallel_webp);
        assert_eq!((r.fps, r.video_quality, r.output_height), (60, 100, 2160));
        let s = Settings::default();
        assert_eq!(
            s.hotkey_record.output,
            OutputKind::Video(VideoContainer::Mkv)
        );
        assert_eq!(
            (s.hotkey_record.fps, s.hotkey_record.video_quality),
            (60, 100)
        );
        assert!(
            s.hotkey_record.record_audio
                && s.hotkey_record.system_audio
                && s.hotkey_record.microphone == MicChoice::Auto
        );
        assert_eq!(s.region_record.output, OutputKind::Anim(AnimFormat::Webp));
        assert_eq!(
            (s.region_record.fps, s.region_record.output_height),
            (15, 480)
        );
        assert_eq!(
            (
                s.region_anim.fps,
                s.region_anim.max_height,
                s.region_anim.webp_quality
            ),
            (15, 480, 80)
        );
        assert_eq!(s.hotkey_screen, ScreenChoice::Primary);
        let h = &s.hotkeys;
        assert!([
            &h.region_screenshot,
            &h.fullscreen_screenshot,
            &h.start_video,
            &h.start_audio,
            &h.stop_recording
        ]
        .iter()
        .all(|k| k.is_empty()));
    }

    #[test]
    fn conversions_follow_the_library_folder_unless_set() {
        let mut s = Settings::default();
        assert_eq!(s.convert_dir(), s.media_dir);
        s.media_dir = "/m".into();
        assert_eq!(
            s.convert_dir(),
            PathBuf::from("/m"),
            "follows the library folder"
        );
        s.convert_dir = Some("/c".into());
        assert_eq!(s.convert_dir(), PathBuf::from("/c"));
        let mut s = Settings::default();
        assert_eq!(
            s.hotkey_audio, s.audio_record,
            "hotkey audio starts like the page"
        );
        s.hotkey_audio.format = AudioFormat::Flac;
        assert_eq!(
            s.audio_profile(RecordProfile::Hotkey).unwrap().format,
            AudioFormat::Flac
        );
        assert_eq!(
            s.audio_profile(RecordProfile::App).unwrap().format,
            AudioFormat::Mp3
        );
        assert!(s.audio_profile(RecordProfile::Region).is_none());
        let a = AudioRecordSettings::default();
        assert_eq!(
            (a.format, a.kbps, a.system_audio),
            (AudioFormat::Mp3, 256, true)
        );
    }

    #[test]
    fn quick_video_profile_rejects_animation_formats() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.toml");
        std::fs::write(
            &path,
            "[hotkey_record]\noutput = { kind = \"anim\", format = \"gif\" }\n",
        )
        .unwrap();
        let s = Settings::load_from(&path);
        assert_eq!(
            s.hotkey_record.output,
            OutputKind::Video(VideoContainer::Mkv)
        );
    }

    #[test]
    fn screen_choice_roundtrip() {
        for c in [
            ScreenChoice::Primary,
            ScreenChoice::UnderCursor,
            ScreenChoice::Display(r"\.DISPLAY2".into()),
        ] {
            let s = Settings {
                hotkey_screen: c.clone(),
                ..Settings::default()
            };
            let back: Settings = toml::from_str(&toml::to_string(&s).unwrap()).unwrap();
            assert_eq!(back.hotkey_screen, c);
        }
    }

    #[test]
    fn after_capture_flags() {
        assert!(AfterCapture::Copy.copies() && !AfterCapture::Copy.saves());
        assert!(AfterCapture::CopyAndSave.copies() && AfterCapture::CopyAndSave.saves());
    }
}
