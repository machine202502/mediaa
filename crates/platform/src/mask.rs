#[cfg(windows)]
pub use crate::windows::mask::ScreenMask;

#[cfg(not(windows))]
pub struct ScreenMask;

#[cfg(not(windows))]
impl ScreenMask {
    pub fn create() -> Option<Self> {
        None
    }

    pub fn show(&self) {}

    pub fn hide(&self) {}

    pub fn pump(&self) {}
}
