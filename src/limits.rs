use serde::{Deserialize, Serialize};

use crate::model::LimitsSnapshot;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_json_bytes: u64,
    pub max_audio_bytes: u64,
    pub max_audio_seconds: f64,
    pub max_total_output_seconds: f64,
    pub max_intervals: usize,
    pub max_versions: usize,
    pub max_interval_refs_per_version: usize,
    pub max_waveform_buckets: usize,
    pub max_evidence_entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_json_bytes: 8 * MIB,
            max_audio_bytes: 2 * GIB,
            max_audio_seconds: 1_800.0,
            max_total_output_seconds: 10_800.0,
            max_intervals: 5_000,
            max_versions: 200,
            max_interval_refs_per_version: 5_000,
            max_waveform_buckets: 4_000,
            max_evidence_entries: 20_000,
        }
    }
}

impl Limits {
    pub fn snapshot(&self) -> LimitsSnapshot {
        LimitsSnapshot {
            max_json_bytes: self.max_json_bytes,
            max_audio_bytes: self.max_audio_bytes,
            max_audio_seconds: self.max_audio_seconds,
            max_total_output_seconds: self.max_total_output_seconds,
            max_intervals: self.max_intervals,
            max_versions: self.max_versions,
            max_interval_refs_per_version: self.max_interval_refs_per_version,
            max_waveform_buckets: self.max_waveform_buckets,
            max_evidence_entries: self.max_evidence_entries,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.max_json_bytes == 0
            || self.max_audio_bytes == 0
            || self.max_audio_seconds <= 0.0
            || self.max_total_output_seconds <= 0.0
            || self.max_intervals == 0
            || self.max_versions == 0
            || self.max_interval_refs_per_version == 0
            || self.max_waveform_buckets == 0
            || self.max_evidence_entries == 0
        {
            return Err("all limits must be greater than zero".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    JsonBytes,
    AudioBytes,
    AudioSeconds,
    Intervals,
    Versions,
    IntervalRefs,
    WaveformBuckets,
    EvidenceEntries,
}

impl LimitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::JsonBytes => "max_json_bytes",
            Self::AudioBytes => "max_audio_bytes",
            Self::AudioSeconds => "max_audio_seconds",
            Self::Intervals => "max_intervals",
            Self::Versions => "max_versions",
            Self::IntervalRefs => "max_interval_refs_per_version",
            Self::WaveformBuckets => "max_waveform_buckets",
            Self::EvidenceEntries => "max_evidence_entries",
        }
    }
}

pub fn check_limit(kind: LimitKind, actual: u64, allowed: u64) -> Result<(), String> {
    if actual > allowed {
        Err(format!(
            "{} exceeded: actual={} allowed={}",
            kind.as_str(),
            actual,
            allowed
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe_and_serializable() {
        let limits = Limits::default();
        assert!(limits.validate().is_ok());
        assert!(limits.max_waveform_buckets <= 10_000);
        assert_eq!(limits.snapshot().max_intervals, 5_000);
    }

    #[test]
    fn zero_limit_is_rejected() {
        let limits = Limits {
            max_intervals: 0,
            ..Limits::default()
        };
        assert!(limits.validate().is_err());
    }

    #[test]
    fn oversized_value_names_the_exact_limit() {
        let error = check_limit(LimitKind::Intervals, 9, 8).unwrap_err();
        assert!(error.contains("max_intervals"));
        assert!(error.contains("actual=9"));
    }
}
