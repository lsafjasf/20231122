//! Offline packet-loss concealment (PLC) evaluation library.
//!
//! Models three network-recording loss types (isolated, burst, one-sided),
//! runs three concealment strategies (silence, repeat-last-frame,
//! interpolation) and scores each gap with duration, waveform energy
//! difference and an audible-artifact flag.

pub mod eval;
pub mod loss;
pub mod metrics;
pub mod plc;
pub mod signal;
pub mod wav;

pub const FRAME_MS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Silence,
    RepeatLastFrame,
    Interpolate,
}

impl Strategy {
    pub const ALL: [Strategy; 3] = [
        Strategy::Silence,
        Strategy::RepeatLastFrame,
        Strategy::Interpolate,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Strategy::Silence => "silence",
            Strategy::RepeatLastFrame => "repeat",
            Strategy::Interpolate => "interpolate",
        }
    }
}

impl std::fmt::Display for Strategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

impl std::str::FromStr for Strategy {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "silence" => Ok(Strategy::Silence),
            "repeat" => Ok(Strategy::RepeatLastFrame),
            "interpolate" | "interp" => Ok(Strategy::Interpolate),
            other => Err(anyhow::anyhow!("unknown strategy: {other}")),
        }
    }
}
