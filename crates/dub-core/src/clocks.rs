//! Two device clocks that are deliberately not synchronised.
//!
//! Device A is the processing node, device B the evidence recorder. Each has
//! its own epoch offset and drift relative to the host monotonic clock, so
//! their raw timestamps disagree. Every evidence record carries *both*
//! timestamps, and paired samples collected at runtime let us (and an
//! independent reviewer) translate one clock domain into the other.

use serde::{Deserialize, Serialize};

/// A device clock defined by an offset and a drift against CLOCK_MONOTONIC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceClock {
    pub name: String,
    /// Constant epoch offset vs host monotonic clock, nanoseconds.
    pub offset_ns: i64,
    /// Linear drift, parts per million.
    pub drift_ppm: f64,
}

impl DeviceClock {
    pub fn new(name: &str, offset_ns: i64, drift_ppm: f64) -> Self {
        Self {
            name: name.into(),
            offset_ns,
            drift_ppm,
        }
    }

    /// Host monotonic time in nanoseconds.
    fn monotonic_ns() -> i64 {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
        ts.tv_sec * 1_000_000_000 + ts.tv_nsec
    }

    /// Current reading of this device clock, nanoseconds on its own scale.
    pub fn now_ns(&self) -> i64 {
        let m = Self::monotonic_ns();
        let drifted = m as f64 * (1.0 + self.drift_ppm / 1e6);
        self.offset_ns + drifted as i64
    }
}

/// One paired observation of both clocks taken (nearly) simultaneously.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ClockSample {
    pub a_ns: i64,
    pub b_ns: i64,
}

/// Synchronisation table between the two device clocks.
///
/// Serialised into the job state and the exported report so that the
/// mapping survives reload and can be replayed by a reviewer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClockSync {
    pub samples: Vec<ClockSample>,
}

impl ClockSync {
    /// Take a paired sample of both clocks.
    pub fn observe(&mut self, a: &DeviceClock, b: &DeviceClock) {
        let a_ns = a.now_ns();
        let b_ns = b.now_ns();
        self.samples.push(ClockSample { a_ns, b_ns });
    }

    /// Estimated offset (b - a) in ns: median over samples, robust to jitter.
    pub fn offset_b_minus_a_ns(&self) -> Option<i64> {
        if self.samples.is_empty() {
            return None;
        }
        let mut deltas: Vec<i64> = self.samples.iter().map(|s| s.b_ns - s.a_ns).collect();
        deltas.sort_unstable();
        Some(deltas[deltas.len() / 2])
    }

    /// Translate a device-A timestamp into the device-B clock domain.
    pub fn map_a_to_b(&self, a_ns: i64) -> Option<i64> {
        self.offset_b_minus_a_ns().map(|off| a_ns + off)
    }

    /// Translate a device-B timestamp into the device-A clock domain.
    pub fn map_b_to_a(&self, b_ns: i64) -> Option<i64> {
        self.offset_b_minus_a_ns().map(|off| b_ns - off)
    }
}

/// The pair of unsynchronised device clocks used across the whole job.
pub struct ClockPair {
    pub a: DeviceClock,
    pub b: DeviceClock,
}

impl Default for ClockPair {
    /// Deterministic skew so runs are reproducible: B lags A by ~3.7 s and
    /// drifts +25 ppm, modelling two devices that never synced.
    fn default() -> Self {
        Self {
            a: DeviceClock::new("device-a/processor", 0, 0.0),
            b: DeviceClock::new("device-b/recorder", -3_700_000_000, 25.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks_disagree_but_mapping_recovers() {
        let pair = ClockPair::default();
        let mut sync = ClockSync::default();
        for _ in 0..5 {
            sync.observe(&pair.a, &pair.b);
        }
        let a = pair.a.now_ns();
        let b = pair.b.now_ns();
        assert_ne!(a, b, "clocks must be unsynchronised");
        let mapped = sync.map_a_to_b(a).unwrap();
        // Mapping error stays well under a millisecond for nearby samples.
        assert!((mapped - b).abs() < 1_000_000, "mapped={mapped} b={b}");
    }
}
