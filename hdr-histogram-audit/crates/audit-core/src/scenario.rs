//! Canonical demonstration scenario used by the source-check page.
//!
//! The fixture is fully deterministic so screenshots, tests and replays always
//! show the same provenance chain.

use crate::model::RawSample;
use crate::pipeline::PipelineConfig;

pub fn default_config() -> PipelineConfig {
    PipelineConfig {
        headroom_nits: 1000.0,
        bin_count: 16,
        edge_tolerance: 1e-4,
    }
}

/// Builds the fixed input batch.
///
/// `s-03` exceeds the headroom and is truncated; `s-04` lands exactly on a bin
/// edge inside the tolerance band and is indeterminate; `s-05` is a non-finite
/// reading and is the first hard error state.
pub fn fixture_samples() -> Vec<RawSample> {
    let config = default_config();
    let width = 1.0 / config.bin_count as f64;
    vec![
        RawSample {
            sample_id: "s-01".to_string(),
            pts_ns: Some(0),
            source_element: "appsink0".to_string(),
            nits: 120.0,
            pixel: Some((12, 40)),
        },
        RawSample {
            sample_id: "s-02".to_string(),
            pts_ns: Some(33_333_333),
            source_element: "appsink0".to_string(),
            nits: 480.0,
            pixel: Some((200, 90)),
        },
        RawSample {
            sample_id: "s-03".to_string(),
            pts_ns: Some(66_666_666),
            source_element: "appsink0".to_string(),
            // 1400 nits > 1000 nits headroom -> truncated.
            nits: 1400.0,
            pixel: Some((512, 512)),
        },
        RawSample {
            sample_id: "s-04".to_string(),
            pts_ns: Some(100_000_000),
            source_element: "appsink0".to_string(),
            // Exactly one bin width above zero: an interior bin edge.
            nits: config.headroom_nits * width,
            pixel: Some((333, 17)),
        },
        RawSample {
            sample_id: "s-05".to_string(),
            pts_ns: Some(133_333_333),
            source_element: "appsink0".to_string(),
            nits: f64::NAN,
            pixel: Some((77, 640)),
        },
        RawSample {
            sample_id: "s-06".to_string(),
            pts_ns: Some(166_666_666),
            source_element: "appsink0".to_string(),
            nits: 760.0,
            pixel: Some((900, 22)),
        },
    ]
}
