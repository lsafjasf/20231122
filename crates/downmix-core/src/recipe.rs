//! Downmix recipes. Each object is rendered twice with distinct, named
//! recipes so reviewers can compare parameters and waveforms.

use crate::error::{DownmixError, Result};
use crate::model::{ProcessingParams, Revision};

/// Canonical GStreamer channel names in positional order.
pub const STEREO: [&str; 2] = ["front-left", "front-right"];

/// Named layouts we know how to fold into stereo.
pub fn supported_layout(channels: usize) -> Option<Vec<String>> {
    let layout: &[&str] = match channels {
        1 => &["mono"],
        2 => &["front-left", "front-right"],
        4 => &[
            "front-left",
            "front-right",
            "rear-left",
            "rear-right",
        ],
        6 => &[
            "front-left",
            "front-right",
            "front-center",
            "lfe",
            "rear-left",
            "rear-right",
        ],
        8 => &[
            "front-left",
            "front-right",
            "front-center",
            "lfe",
            "rear-left",
            "rear-right",
            "side-left",
            "side-right",
        ],
        _ => return None,
    };
    Some(layout.iter().map(|s| (*s).to_string()).collect())
}

struct Weights {
    center: f64,
    surround: f64,
    lfe: f64,
}

/// Legacy broadcast-style recipe: fixed 3 dB center attenuation and surround
/// kept at full level with no LFE contribution.
fn legacy_weights() -> Weights {
    Weights {
        center: 0.7071,
        surround: 1.0,
        lfe: 0.0,
    }
}

/// Current ITU-R BS.775-style stereo downmix: center at -3 dB split equally,
/// surrounds at -3 dB, LFE dropped with a documented zero coefficient.
fn current_weights() -> Weights {
    Weights {
        center: 0.7071,
        surround: 0.7071,
        lfe: 0.0,
    }
}

fn build_matrix(revision: Revision, layout: &[String]) -> Vec<f64> {
    let weights = match revision {
        Revision::Legacy => legacy_weights(),
        Revision::Current => current_weights(),
    };
    let in_ch = layout.len();
    let mut matrix = vec![0.0f64; 2 * in_ch];
    for (idx, name) in layout.iter().enumerate() {
        match name.as_str() {
            "front-left" => {
                matrix[0 * in_ch + idx] = 1.0;
            }
            "front-right" => {
                matrix[1 * in_ch + idx] = 1.0;
            }
            "mono" => {
                matrix[0 * in_ch + idx] = 0.7071;
                matrix[1 * in_ch + idx] = 0.7071;
            }
            "front-center" => {
                let half = weights.center * 0.7071;
                matrix[0 * in_ch + idx] = half;
                matrix[1 * in_ch + idx] = half;
            }
            "lfe" => {
                matrix[0 * in_ch + idx] = weights.lfe;
                matrix[1 * in_ch + idx] = weights.lfe;
            }
            "rear-left" | "side-left" => {
                matrix[0 * in_ch + idx] = weights.surround;
            }
            "rear-right" | "side-right" => {
                matrix[1 * in_ch + idx] = weights.surround;
            }
            _ => {}
        }
    }
    matrix
}

/// Build the fully described parameter set for one revision.
pub fn build_params(
    revision: Revision,
    input_channels: usize,
    sample_rate: u32,
    sample_format: &str,
) -> Result<ProcessingParams> {
    let layout = supported_layout(input_channels).ok_or_else(|| {
        DownmixError::Stream(format!(
            "no downmix layout registered for {input_channels} channels"
        ))
    })?;
    let matrix = build_matrix(revision, &layout);
    let recipe = match revision {
        Revision::Legacy => "legacy-broadcast-v1",
        Revision::Current => "itu-bs775-stereo-v2",
    };
    let pipeline = pipeline_description(revision, input_channels);
    Ok(ProcessingParams {
        revision,
        recipe: recipe.to_string(),
        input_channels,
        output_channels: 2,
        input_layout: layout,
        output_layout: STEREO.iter().map(|s| (*s).to_string()).collect(),
        coefficient_matrix: matrix,
        sample_rate,
        sample_format: sample_format.to_string(),
        pipeline_description: pipeline,
    })
}

/// Human-auditable GStreamer pipeline shape for the manifest. The runtime in
/// [`crate::pipeline`] constructs the same graph programmatically.
fn pipeline_description(revision: Revision, in_ch: usize) -> String {
    format!(
        "filesrc ! decodebin ! audioconvert ! audioresample ! \
         capsfilter(audio/x-raw,channels={in_ch}) ! \
         audiomixmatrix(matrix=<recipe:{recipe}>,out-channels=2) ! \
         audioconvert ! wavenc ! filesink",
        recipe = revision.label()
    )
}
