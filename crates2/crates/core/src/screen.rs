use crate::geometry::Rect;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowState {
    Available,
    Minimized,
    Hidden,
    Missing,
    Mismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorState {
    Available,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MonitorId(u32);

impl MonitorId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CaptureWindowId(u32);

impl CaptureWindowId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub id: MonitorId,
    pub device: String,
    pub index: usize,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub primary: bool,
}

impl MonitorInfo {
    pub fn label(&self) -> String {
        format!("{} ({}×{})", self.name, self.width, self.height)
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: CaptureWindowId,
    pub title: String,
    pub app: String,
    pub pid: u32,
    pub width: u32,
    pub height: u32,
}

impl WindowInfo {
    pub fn label(&self) -> String {
        if self.app.is_empty() || self.title.contains(&self.app) {
            self.title.clone()
        } else {
            format!("{} — {}", self.title, self.app)
        }
    }
}

impl WindowState {
    pub fn is_available(self) -> bool {
        self == Self::Available
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor() -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::new(1),
            device: r"\\.\DISPLAY1".into(),
            index: 0,
            name: "DELL U2720Q".into(),
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
            scale: 1.5,
            primary: true,
        }
    }

    fn window() -> WindowInfo {
        WindowInfo {
            id: CaptureWindowId::new(42),
            title: "Document — Visual Studio Code".into(),
            app: "Visual Studio Code".into(),
            pid: 1234,
            width: 1600,
            height: 900,
        }
    }

    #[test]
    fn labels_and_bounds() {
        assert_eq!(monitor().label(), "DELL U2720Q (3840×2160)");
        assert_eq!(monitor().bounds(), Rect::new(0, 0, 3840, 2160));
        assert_eq!(window().label(), "Document — Visual Studio Code");
        assert!(WindowState::Available.is_available());
        assert!(!WindowState::Minimized.is_available());
    }

    #[test]
    fn protocol_types_survive_serialization() {
        let back: MonitorInfo = toml::from_str(&toml::to_string(&monitor()).unwrap()).unwrap();
        assert_eq!(back, monitor());
        let back: WindowInfo = toml::from_str(&toml::to_string(&window()).unwrap()).unwrap();
        assert_eq!(back, window());
        let wrap: Wrap<MonitorState> = toml::from_str("state = \"disconnected\"").unwrap();
        assert_eq!(wrap.state, MonitorState::Disconnected);
        let wrap: Wrap<WindowState> = toml::from_str("state = \"available\"").unwrap();
        assert_eq!(wrap.state, WindowState::Available);
    }

    #[derive(Deserialize)]
    struct Wrap<T> {
        state: T,
    }
}
