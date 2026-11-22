use std::fmt;

/// Rational frame rate (frames per second), e.g. 24/1, 25/1, 30/1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameRate {
    pub num: u32,
    pub den: u32,
}

impl FrameRate {
    pub const FPS_24: Self = Self::new(24, 1);
    pub const FPS_25: Self = Self::new(25, 1);
    pub const FPS_30: Self = Self::new(30, 1);

    pub const fn new(num: u32, den: u32) -> Self {
        assert!(num > 0 && den > 0, "frame rate must be positive");
        Self { num, den }
    }

    /// Nanoseconds covered by a single frame.
    pub fn frame_duration_ns(&self) -> u64 {
        1_000_000_000u64 * self.den as u64 / self.num as u64
    }

    /// Convert a frame index at this rate to the unified nanosecond timeline.
    pub fn frame_to_ns(&self, frame: u64) -> u64 {
        // Multiply first, divide once: exact for the integer frame rates used here.
        frame * 1_000_000_000u64 * self.den as u64 / self.num as u64
    }

    /// Render a frame index as HH:MM:SS.mmm timecode at this rate.
    pub fn frame_to_timecode(&self, frame: u64) -> String {
        crate::fmt::ns_to_timecode(self.frame_to_ns(frame))
    }
}

impl fmt::Display for FrameRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}fps", self.num)
        } else {
            write!(f, "{}/{}fps", self.num, self.den)
        }
    }
}
