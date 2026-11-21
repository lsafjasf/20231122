//! Packet-loss models and gap bookkeeping.
//!
//! A recording is sliced into fixed-size frames (20 ms). A loss pattern
//! marks frames as lost; contiguous lost frames form a `Gap`. Lost frames
//! are zeroed in the degraded signal (what a jitter buffer would emit
//! without concealment).

use crate::wav::Wav;
use crate::FRAME_MS;
use anyhow::{Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossKind {
    /// Single isolated lost frames scattered through the stream.
    Isolated,
    /// Long runs of consecutive lost frames.
    Burst,
    /// Only one channel (one talker side) loses frames; the other
    /// channel stays intact (stereo two-talker recording).
    OneSided,
}

impl LossKind {
    pub const ALL: [LossKind; 3] = [LossKind::Isolated, LossKind::Burst, LossKind::OneSided];

    pub fn label(self) -> &'static str {
        match self {
            LossKind::Isolated => "isolated",
            LossKind::Burst => "burst",
            LossKind::OneSided => "one-sided",
        }
    }
}

impl std::fmt::Display for LossKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// One contiguous run of lost frames.
#[derive(Debug, Clone)]
pub struct Gap {
    /// First lost frame index.
    pub start_frame: usize,
    /// Number of consecutive lost frames.
    pub frames: usize,
    /// Channel affected (for one-sided loss in stereo files).
    pub channel: usize,
}

impl Gap {
    pub fn duration_ms(&self, frame_ms: usize) -> usize {
        self.frames * frame_ms
    }
}

/// A loss manifest: which frames were dropped, on which channel.
#[derive(Debug, Clone)]
pub struct LossManifest {
    pub kind: LossKind,
    pub frame_ms: usize,
    pub gaps: Vec<Gap>,
}

impl LossManifest {
    /// Build a deterministic loss pattern for `total_frames`.
    pub fn generate(kind: LossKind, total_frames: usize, channel: usize) -> Self {
        let mut gaps = Vec::new();
        match kind {
            LossKind::Isolated => {
                // one lost frame every ~17 frames, offset by a small pattern
                let mut f = 5usize;
                while f < total_frames.saturating_sub(2) {
                    gaps.push(Gap {
                        start_frame: f,
                        frames: 1,
                        channel,
                    });
                    f += 17 + (f * 7) % 5;
                }
            }
            LossKind::Burst => {
                // bursts of 4-9 frames
                let mut f = 8usize;
                let mut i = 0usize;
                while f < total_frames.saturating_sub(10) {
                    let len = 4 + (i * 3) % 6;
                    gaps.push(Gap {
                        start_frame: f,
                        frames: len,
                        channel,
                    });
                    f += len + 23 + (i * 11) % 9;
                    i += 1;
                }
                // trailing dropout: the recording ends mid-burst, so no
                // future context exists for the last gap
                if total_frames > 40 {
                    gaps.push(Gap {
                        start_frame: total_frames - 6,
                        frames: 6,
                        channel,
                    });
                }
            }
            LossKind::OneSided => {
                // medium runs on one side only
                let mut f = 6usize;
                let mut i = 0usize;
                while f < total_frames.saturating_sub(6) {
                    let len = 2 + (i * 5) % 4;
                    gaps.push(Gap {
                        start_frame: f,
                        frames: len,
                        channel,
                    });
                    f += len + 13 + (i * 7) % 6;
                    i += 1;
                }
            }
        }
        LossManifest {
            kind,
            frame_ms: FRAME_MS,
            gaps,
        }
    }

    /// Channels a gap affects. Stream loss (isolated/burst) hits every
    /// channel; one-sided loss hits only its recorded channel.
    pub fn affected_channels(&self, gap: &Gap, n_channels: usize) -> Vec<usize> {
        match self.kind {
            LossKind::OneSided => vec![gap.channel.min(n_channels.saturating_sub(1))],
            _ => (0..n_channels).collect(),
        }
    }

    /// Apply the manifest to a clean signal, zeroing lost frames.
    pub fn degrade(&self, clean: &Wav) -> Wav {
        let rate = clean.spec.sample_rate as usize;
        let ch = clean.spec.channels;
        let frame_len = rate * self.frame_ms / 1000;
        let mut out = clean.clone();
        for gap in &self.gaps {
            let start = gap.start_frame * frame_len;
            let end = ((gap.start_frame + gap.frames) * frame_len).min(out.frames());
            for c in self.affected_channels(gap, ch) {
                for frame in start..end {
                    out.samples[frame * ch + c] = 0.0;
                }
            }
        }
        out
    }

    /// Serialize to a simple text manifest.
    pub fn to_text(&self) -> String {
        let mut s = format!("kind={} frame_ms={}\n", self.kind.label(), self.frame_ms);
        s.push_str("start_frame,frames,channel\n");
        for g in &self.gaps {
            s.push_str(&format!("{},{},{}\n", g.start_frame, g.frames, g.channel));
        }
        s
    }

    pub fn from_text(text: &str) -> Result<Self> {
        let mut lines = text.lines();
        let header = lines.next().context("empty manifest")?;
        let mut kind = None;
        let mut frame_ms = FRAME_MS;
        for kv in header.split_whitespace() {
            if let Some((k, v)) = kv.split_once('=') {
                match k {
                    "kind" => {
                        kind = Some(match v {
                            "isolated" => LossKind::Isolated,
                            "burst" => LossKind::Burst,
                            "one-sided" => LossKind::OneSided,
                            other => anyhow::bail!("unknown loss kind {other}"),
                        })
                    }
                    "frame_ms" => frame_ms = v.parse().context("bad frame_ms")?,
                    _ => {}
                }
            }
        }
        let kind = kind.context("manifest missing kind")?;
        let mut gaps = Vec::new();
        for line in lines {
            let line = line.trim();
            if line.is_empty() || line.starts_with("start_frame") {
                continue;
            }
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() != 3 {
                anyhow::bail!("bad gap line: {line}");
            }
            gaps.push(Gap {
                start_frame: parts[0].parse().context("bad start_frame")?,
                frames: parts[1].parse().context("bad frames")?,
                channel: parts[2].parse().context("bad channel")?,
            });
        }
        Ok(LossManifest {
            kind,
            frame_ms,
            gaps,
        })
    }
}
