use crate::cursor::CursorState;
use crate::cursor_overlay::{arrow, CursorImage};
use device_query::{DeviceQuery, DeviceState};
use std::sync::OnceLock;

pub(crate) fn position() -> Option<(i32, i32)> {
    Some(DeviceState::new().get_mouse().coords)
}

pub(crate) fn current() -> Option<CursorState> {
    static ARROW: OnceLock<CursorImage> = OnceLock::new();
    let (x, y) = position()?;
    Some(CursorState {
        x,
        y,
        image: ARROW.get_or_init(|| arrow(1.0)).clone(),
    })
}
