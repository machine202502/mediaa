#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntegerLimit {
    pub min: u32,
    pub max: u32,
}

impl IntegerLimit {
    pub const fn contains(self, value: u32) -> bool {
        value >= self.min && value <= self.max
    }

    pub const fn clamp(self, value: u32) -> u32 {
        if value < self.min {
            self.min
        } else if value > self.max {
            self.max
        } else {
            value
        }
    }
}

pub const FPS: IntegerLimit = IntegerLimit { min: 1, max: 60 };
pub const VIDEO_QUALITY: IntegerLimit = IntegerLimit { min: 1, max: 100 };
pub const GIF_COLORS: IntegerLimit = IntegerLimit { min: 4, max: 256 };
pub const AUDIO_KBPS: IntegerLimit = IntegerLimit { min: 32, max: 320 };
pub const DEFAULT_VIDEO_AUDIO_KBPS: u32 = 192;
pub const DEFAULT_MUSIC_AUDIO_KBPS: u32 = 256;

#[cfg(test)]
mod tests {
    use super::{IntegerLimit, FPS};

    #[test]
    fn limits_are_inclusive() {
        assert!(FPS.contains(1));
        assert!(FPS.contains(60));
        assert!(!FPS.contains(0));
        assert!(!FPS.contains(61));
        assert_eq!(FPS.clamp(120), 60);
        assert_eq!(IntegerLimit { min: 5, max: 9 }.clamp(3), 5);
    }
}
