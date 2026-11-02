use mediaa_core::Rect;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Available,
    Minimized,
    Hidden,
    Missing,
    Mismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
