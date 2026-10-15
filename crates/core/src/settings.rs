//! User settings, persisted as TOML.

use crate::formats::{AudioFormat, ImageFormat, OutputKind};
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

/// What happens right after a screenshot is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AfterCapture {
    Copy,
    Save,
    #[default]
    CopyAndSave,
}

impl AfterCapture {
    pub const ALL: [AfterCapture; 3] = [AfterCapture::Copy, AfterCapture::Save, AfterCapture::CopyAndSave];

    pub fn copies(self) -> bool {
        matches!(self, AfterCapture::Copy | AfterCapture::CopyAndSave)
    }

    pub fn saves(self) -> bool {
        matches!(self, AfterCapture::Save | AfterCapture::CopyAndSave)
    }
}

/// `paletteuse` dithering for GIF output.
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
    /// Hide the main window before capturing so it does not appear in the shot.
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

/// Microphone for recordings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MicChoice {
    /// The system default input device (whichever is connected when recording starts).
    #[default]
    Auto,
    /// No microphone.
    Off,
    /// A device by name; if it is not connected, the default one is used instead.
    Device(String),
}

impl MicChoice {
    /// Device to record from, given the connected `devices` and the system `default` one.
    pub fn resolve(&self, devices: &[String], default: Option<&str>) -> Option<String> {
        let fallback = || default.map(str::to_string).or_else(|| devices.first().cloned());
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
    /// Master switch for sound; off records silent videos whatever the sources below say.
    pub record_audio: bool,
    pub fps: u32,
    /// 1–100, mapped to the encoder's quality scale (x264 CRF).
    pub video_quality: u8,
    pub microphone: MicChoice,
    /// Record what the computer plays (loopback) where the OS supports it.
    pub system_audio: bool,
    /// Several sources (displays / windows): the sound goes into every file, not only the first.
    pub audio_in_every_file: bool,
    /// Draw the mouse cursor into the recording.
    pub record_cursor: bool,
    /// Output video height in pixels (aspect ratio kept, never upscaled); 0 = original.
    pub output_height: u32,
    pub hide_to_tray: bool,
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
            output_height: 1080,
            hide_to_tray: true,
        }
    }
}

/// Audio recording (the "Record audio" page).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioRecordSettings {
    pub format: AudioFormat,
    /// Bitrate of lossy formats, kbit/s.
    pub kbps: u32,
    pub microphone: MicChoice,
    pub system_audio: bool,
}

impl Default for AudioRecordSettings {
    fn default() -> Self {
        Self { format: AudioFormat::Mp3, kbps: 192, microphone: MicChoice::Auto, system_audio: true }
    }
}

/// Bitrates offered for lossy audio (kbit/s).
pub const AUDIO_BITRATES: [u32; 5] = [96, 128, 192, 256, 320];

/// Output height presets offered for recordings (4K, QHD, FHD, HD, SD).
pub const OUTPUT_HEIGHTS: [u32; 5] = [2160, 1440, 1080, 720, 480];

/// Height presets for GIF / WebP animations.
pub const ANIM_HEIGHTS: [u32; 5] = [1080, 720, 480, 360, 240];

impl RecordSettings {
    /// Height limit for the encoder, `None` = keep the source size.
    pub fn max_height(&self) -> Option<u32> {
        (self.output_height > 0).then_some(self.output_height)
    }

    pub fn records_audio(&self) -> bool {
        self.output.supports_audio() && self.record_audio && (self.microphone != MicChoice::Off || self.system_audio)
    }
}

/// Settings for GIF / animated WebP output, used both for recordings and conversions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimSettings {
    pub fps: u32,
    /// Downscale to this height (keeping aspect, never upscaling); 0 keeps the source size.
    pub max_height: u32,
    pub gif_colors: u32,
    pub gif_dither: GifDither,
    /// libwebp quality 0–100.
    pub webp_quality: u8,
}

impl Default for AnimSettings {
    fn default() -> Self {
        Self {
            fps: 15,
            // 480p: the usual size of GIFs / WebPs for chats and web pages.
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
    /// Starts a video recording (hotkey settings, configured display).
    pub start_video: String,
    /// Starts an audio recording.
    pub start_audio: String,
    /// Stops whichever recording runs.
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

/// Display recorded by the hotkey / tray "start recording".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScreenChoice {
    #[default]
    Primary,
    /// The display the mouse cursor is on.
    UnderCursor,
    /// A specific display by its stable system device name.
    Display(String),
}

/// Which recording settings apply: the ones used from the app window (Record page, its area
/// picker) or the separate ones for the global hotkey / tray area picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordProfile {
    App,
    Hotkey,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemePreference,
    /// UI language code ("en", "ru"…); `None` follows the system language.
    pub language: Option<String>,
    /// Where screenshots and recordings are saved (and what the library shows).
    pub media_dir: PathBuf,
    /// Default folder for conversion results; `None` = the library folder (follows it).
    pub convert_dir: Option<PathBuf>,
    pub screenshot: ScreenshotSettings,
    pub record: RecordSettings,
    pub anim: AnimSettings,
    /// Recording settings used by the global hotkey and the tray area picker.
    pub hotkey_record: RecordSettings,
    pub hotkey_anim: AnimSettings,
    pub hotkey_screen: ScreenChoice,
    /// Audio recording from the "Record audio" page.
    pub audio_record: AudioRecordSettings,
    /// Audio recording started by the hotkey or from the tray.
    pub hotkey_audio: AudioRecordSettings,
    pub hotkeys: HotkeySettings,
    /// Agent section: providers and models.
    pub agent: crate::agent::AgentSettings,
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
            hotkey_record: RecordSettings::default(),
            hotkey_anim: AnimSettings::default(),
            hotkey_screen: ScreenChoice::Primary,
            audio_record: AudioRecordSettings::default(),
            hotkey_audio: AudioRecordSettings::default(),
            hotkeys: HotkeySettings::default(),
            agent: crate::agent::AgentSettings::default(),
        }
    }
}

impl Settings {
    /// Loads settings; a missing or unreadable file yields defaults (logged, never fatal).
    pub fn load_from(path: &Path) -> Settings {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str::<Settings>(&text) {
                Ok(s) => s.sanitized(),
                Err(e) => {
                    log::warn!("settings: {} is invalid ({e}); using defaults", path.display());
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
        // Write-then-rename so a crash never leaves a half-written file behind.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// The user's settings, with provider keys from the OS keychain.
    pub fn load() -> Settings {
        let mut s = Self::load_from(&paths::settings_file());
        crate::secrets::load(&mut s.agent);
        s
    }

    /// Saves the user's settings; provider keys go to the OS keychain.
    pub fn save(&self) -> anyhow::Result<()> {
        crate::secrets::save(&self.agent)?;
        self.save_to(&paths::settings_file())
    }

    /// Clamps numeric values into supported ranges (hand-edited files may contain anything).
    pub fn sanitized(mut self) -> Settings {
        for r in [&mut self.record, &mut self.hotkey_record] {
            r.fps = r.fps.clamp(1, 60);
            r.video_quality = r.video_quality.clamp(1, 100);
        }
        for a in [&mut self.anim, &mut self.hotkey_anim] {
            a.fps = a.fps.clamp(1, 50);
            a.gif_colors = a.gif_colors.clamp(4, 256);
            a.webp_quality = a.webp_quality.min(100);
        }
        if self.media_dir.as_os_str().is_empty() {
            self.media_dir = paths::default_media_dir();
        }
        self
    }

    /// Folder conversions go to by default.
    pub fn convert_dir(&self) -> PathBuf {
        self.convert_dir.clone().unwrap_or_else(|| self.media_dir.clone())
    }

    /// Audio recording settings of a profile.
    pub fn audio_profile(&self, p: RecordProfile) -> &AudioRecordSettings {
        match p {
            RecordProfile::App => &self.audio_record,
            RecordProfile::Hotkey => &self.hotkey_audio,
        }
    }

    /// Recording and animation settings of a profile.
    pub fn profile(&self, p: RecordProfile) -> (&RecordSettings, &AnimSettings) {
        match p {
            RecordProfile::App => (&self.record, &self.anim),
            RecordProfile::Hotkey => (&self.hotkey_record, &self.hotkey_anim),
        }
    }

    pub fn profile_mut(&mut self, p: RecordProfile) -> (&mut RecordSettings, &mut AnimSettings) {
        match p {
            RecordProfile::App => (&mut self.record, &mut self.anim),
            RecordProfile::Hotkey => (&mut self.hotkey_record, &mut self.hotkey_anim),
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
        assert_eq!(s.record.video_quality, RecordSettings::default().video_quality);
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
        let mut r = RecordSettings { system_audio: false, microphone: MicChoice::Off, ..RecordSettings::default() };
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
        assert_eq!(MicChoice::Auto.resolve(&devs, Some("Webcam mic")), Some("Webcam mic".into()));
        assert_eq!(MicChoice::Auto.resolve(&devs, None), Some("Headset".into()), "no default: first device");
        assert_eq!(MicChoice::Auto.resolve(&[], None), None, "nothing connected");
        assert_eq!(MicChoice::Off.resolve(&devs, Some("Headset")), None, "explicit None stays off");
        let headset = MicChoice::Device("Headset".into());
        assert_eq!(headset.resolve(&devs, Some("Webcam mic")), Some("Headset".into()));
        let gone = MicChoice::Device("USB Mic".into());
        assert_eq!(gone.resolve(&devs, Some("Webcam mic")), Some("Webcam mic".into()), "unplugged: default instead");
    }

    #[test]
    fn recording_defaults() {
        let r = RecordSettings::default();
        assert_eq!(r.output, OutputKind::Video(VideoContainer::Mp4));
        assert!(r.record_audio && r.system_audio && r.microphone == MicChoice::Auto && r.audio_in_every_file && r.record_cursor);
        assert_eq!((r.fps, r.video_quality, r.output_height), (60, 100, 1080));
        let s = Settings::default();
        assert_eq!(s.hotkey_screen, ScreenChoice::Primary);
        let h = &s.hotkeys;
        assert!([&h.region_screenshot, &h.fullscreen_screenshot, &h.start_video, &h.start_audio, &h.stop_recording].iter().all(|k| k.is_empty()));
    }

    #[test]
    fn conversions_follow_the_library_folder_unless_set() {
        let mut s = Settings::default();
        assert_eq!(s.convert_dir(), s.media_dir);
        s.media_dir = "/m".into();
        assert_eq!(s.convert_dir(), PathBuf::from("/m"), "follows the library folder");
        s.convert_dir = Some("/c".into());
        assert_eq!(s.convert_dir(), PathBuf::from("/c"));
        let mut s = Settings::default();
        assert_eq!(s.hotkey_audio, s.audio_record, "hotkey audio starts like the page");
        s.hotkey_audio.format = AudioFormat::Flac;
        assert_eq!(s.audio_profile(RecordProfile::Hotkey).format, AudioFormat::Flac);
        assert_eq!(s.audio_profile(RecordProfile::App).format, AudioFormat::Mp3);
        let a = AudioRecordSettings::default();
        assert_eq!((a.format, a.kbps, a.system_audio), (AudioFormat::Mp3, 192, true));
    }

    #[test]
    fn screen_choice_roundtrip() {
        for c in [ScreenChoice::Primary, ScreenChoice::UnderCursor, ScreenChoice::Display(r"\.DISPLAY2".into())] {
            let s = Settings { hotkey_screen: c.clone(), ..Settings::default() };
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
