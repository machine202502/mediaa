use crate::geometry::Rect;
use crate::screen::{MonitorInfo, WindowInfo};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordTarget {
    Audio,
    Screens { monitors: Vec<MonitorInfo> },
    Region { monitor: MonitorInfo, region: Rect },
    Windows { windows: Vec<WindowInfo> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopSource {
    Ui,
    Tray,
    Hotkey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopAction {
    Save,
    Discard,
}
