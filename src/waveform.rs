use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::model::{
    ANCHOR_A_NS, ANCHOR_B_NS, ANCHOR_C_NS, FrameSample, Params, ParsedEvent, ParsedLog,
};

pub fn params_sha256(params: &Params) -> String {
    let bytes = serde_json::to_vec(params).expect("params serialize");
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

pub struct TrajectorySampler<'a> {
    events: &'a [ParsedEvent],
    duplicate_counts: Vec<usize>,
    params: &'a Params,
    params_sha256: String,
    width: u32,
    height: u32,
    previous: Option<FrameSample>,
}

impl<'a> TrajectorySampler<'a> {
    pub fn new(log: &'a ParsedLog, params: &'a Params, width: u32, height: u32) -> Self {
        Self {
            events: &log.events,
            duplicate_counts: log.duplicates.iter().map(Vec::len).collect(),
            params,
            params_sha256: params_sha256(params),
            width,
            height,
            previous: None,
        }
    }

    pub fn set_dimensions(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    pub fn sample(&mut self, frame_index: u64, pts_ns: u64, duration_ns: u64) -> FrameSample {
        let mut sample = FrameSample {
            frame_index,
            pts_ns,
            duration_ns,
            track_id: self.params.track_id,
            visible: false,
            raw_x: None,
            raw_y: None,
            x: None,
            y: None,
            clamp_x: false,
            clamp_y: false,
            speed_px_s: 0.0,
            event_seq: None,
            event_ts_ns: None,
            raw_offset: None,
            event_gap_ns: None,
            flags: Vec::new(),
            anchor: anchor_for(pts_ns),
            params_sha256: self.params_sha256.clone(),
        };

        let Some(selected) = self.select(pts_ns, &mut sample) else {
            sample.flags.push("stale_hidden".into());
            self.previous = Some(sample.clone());
            return sample;
        };
        sample.raw_x = Some(selected.x);
        sample.raw_y = Some(selected.y);
        sample.event_seq = Some(selected.seq);
        sample.event_ts_ns = Some(selected.ts_ns);
        sample.raw_offset = Some(selected.raw_offset);
        sample.event_gap_ns = Some(pts_ns.saturating_sub(selected.ts_ns));
        if selected.raw_order_inversion {
            sample.flags.push("raw_order_inversion".into());
        }
        if self
            .duplicate_counts
            .get(selected_index(self.events, &selected))
            .copied()
            > Some(1)
        {
            sample.flags.push("duplicate_timestamp".into());
        }

        let max_x = self.width.saturating_sub(1) as f64;
        let max_y = self.height.saturating_sub(1) as f64;
        let x = selected.x.clamp(0.0, max_x);
        let y = selected.y.clamp(0.0, max_y);
        sample.clamp_x = (x - selected.x).abs() > f64::EPSILON;
        sample.clamp_y = (y - selected.y).abs() > f64::EPSILON;
        if sample.clamp_x || sample.clamp_y {
            sample.flags.push("clamped_to_frame".into());
            if sample.clamp_x {
                sample.flags.push("clamp_x".into());
            }
            if sample.clamp_y {
                sample.flags.push("clamp_y".into());
            }
        }
        sample.x = Some(x);
        sample.y = Some(y);
        sample.visible = true;

        if let Some(previous) = &self.previous
            && previous.visible
            && let (Some(px), Some(py), Some(cx), Some(cy)) =
                (previous.x, previous.y, sample.x, sample.y)
        {
            let dt = pts_ns.saturating_sub(previous.pts_ns) as f64 / 1e9;
            if dt > 0.0 {
                sample.speed_px_s = ((cx - px).powi(2) + (cy - py).powi(2)).sqrt() / dt;
            }
        }
        self.previous = Some(sample.clone());
        sample
    }

    fn select(&self, pts_ns: u64, sample: &mut FrameSample) -> Option<ParsedEvent> {
        if self.events.is_empty() {
            return None;
        }
        let previous_index = self.events.partition_point(|event| event.ts_ns <= pts_ns);
        if previous_index == 0 {
            return None;
        }
        let previous = &self.events[previous_index - 1];
        if previous.ts_ns == pts_ns {
            return Some(previous.clone());
        }
        let next = self.events.get(previous_index);
        if let Some(next) = next {
            let span = next.ts_ns.saturating_sub(previous.ts_ns);
            if span <= self.params.max_gap_ns
                && pts_ns.saturating_sub(previous.ts_ns) <= self.params.max_gap_ns
            {
                let fraction = (pts_ns - previous.ts_ns) as f64 / span as f64;
                let mut interpolated = previous.clone();
                interpolated.x = previous.x + (next.x - previous.x) * fraction;
                interpolated.y = previous.y + (next.y - previous.y) * fraction;
                sample.flags.push("interpolated".into());
                return Some(interpolated);
            }
        }
        if pts_ns.saturating_sub(previous.ts_ns) <= self.params.hold_ns {
            sample.flags.push("held_stale".into());
            return Some(previous.clone());
        }
        None
    }
}

fn selected_index(events: &[ParsedEvent], selected: &ParsedEvent) -> usize {
    events.partition_point(|event| {
        (event.ts_ns, event.raw_offset) < (selected.ts_ns, selected.raw_offset)
    })
}

fn anchor_for(pts_ns: u64) -> Option<String> {
    if pts_ns == ANCHOR_A_NS {
        Some("A".into())
    } else if pts_ns == ANCHOR_B_NS {
        Some("B".into())
    } else if pts_ns == ANCHOR_C_NS {
        Some("C".into())
    } else {
        None
    }
}

pub fn write_trace(path: &Path, samples: &[FrameSample]) -> anyhow::Result<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    for sample in samples {
        serde_json::to_writer(&mut writer, sample)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    Ok(())
}

pub fn write_csv(path: &Path, samples: &[FrameSample]) -> anyhow::Result<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    writeln!(
        writer,
        "frame_index,pts_ns,time_s,visible,raw_x,raw_y,x,y,clamp_x,clamp_y,speed_px_s,event_seq,event_ts_ns,raw_offset,flags,anchor"
    )?;
    for sample in samples {
        writeln!(
            writer,
            "{},{},{:.3},{},{},{},{},{},{},{},{:.3},{},{},{},{},{}",
            sample.frame_index,
            sample.pts_ns,
            sample.pts_ns as f64 / 1e9,
            sample.visible,
            optional_number(sample.raw_x),
            optional_number(sample.raw_y),
            optional_number(sample.x),
            optional_number(sample.y),
            sample.clamp_x,
            sample.clamp_y,
            sample.speed_px_s,
            optional_u64(sample.event_seq),
            optional_u64(sample.event_ts_ns),
            optional_u64(sample.raw_offset),
            sample.flags.join("|"),
            sample.anchor.clone().unwrap_or_default(),
        )?;
    }
    writer.flush()?;
    Ok(())
}

fn optional_number(value: Option<f64>) -> String {
    value.map_or_else(String::new, |number| format!("{number:.3}"))
}

fn optional_u64(value: Option<u64>) -> String {
    value.map_or_else(String::new, |number| number.to_string())
}

pub fn write_svg(path: &Path, samples: &[FrameSample], params: &Params) -> anyhow::Result<()> {
    let _width = 1200.0;
    let _height = 720.0;
    let left = 74.0;
    let right = 1160.0;
    let plot_width = right - left;
    let duration_ns = samples
        .last()
        .map_or(1, |sample| sample.pts_ns + sample.duration_ns)
        .max(1);
    let max_speed = samples
        .iter()
        .map(|sample| sample.speed_px_s)
        .fold(1.0_f64, f64::max)
        .ceil();
    let x_at = |pts_ns: u64| left + (pts_ns as f64 / duration_ns as f64) * plot_width;
    let mut svg = String::new();
    svg.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="720" viewBox="0 0 1200 720">
<rect width="1200" height="720" fill="#0f1720"/>
<text x="74" y="32" fill="#e6edf3" font-family="monospace" font-size="20">mouse trajectory waveform - deterministic rebuild</text>
<text x="74" y="55" fill="#8b98a5" font-family="monospace" font-size="12">track {track} | dedupe={dedupe} | hold={hold_ms}ms | max_gap={gap_ms}ms | params={hash:.16}</text>
"##,
        track = params.track_id,
        dedupe = params.dedupe_policy,
        hold_ms = params.hold_ns / 1_000_000,
        gap_ms = params.max_gap_ns / 1_000_000,
        hash = params_sha256(params),
    ));
    draw_panel(
        &mut svg,
        samples,
        "X position (px)",
        left,
        86.0,
        plot_width,
        160.0,
        0.0,
        320.0,
        x_at,
        |sample| sample.x,
        "#42d9c8",
    );
    draw_panel(
        &mut svg,
        samples,
        "Y position (px)",
        left,
        286.0,
        plot_width,
        160.0,
        180.0,
        0.0,
        x_at,
        |sample| sample.y,
        "#ffb454",
    );
    draw_panel(
        &mut svg,
        samples,
        "Speed (px/s)",
        left,
        486.0,
        plot_width,
        160.0,
        max_speed,
        0.0,
        x_at,
        |sample| Some(sample.speed_px_s),
        "#ed6a9a",
    );
    for (id, pts, color) in [
        ("A duplicate ts", ANCHOR_A_NS, "#7aa2f7"),
        ("B raw inversion", ANCHOR_B_NS, "#c099ff"),
        ("C clamp oob", ANCHOR_C_NS, "#f7768e"),
    ] {
        let x = x_at(pts);
        svg.push_str(&format!(
            r#"<line x1="{x:.2}" y1="76" x2="{x:.2}" y2="670" stroke="{color}" stroke-width="1.5" stroke-dasharray="5 5"/>
<rect x="{x:.2}" y="672" width="132" height="22" rx="3" fill="{color}" opacity="0.18"/>
<text x="{:.2}" y="687" fill="{color}" font-family="monospace" font-size="12">{id} @ {:.3}s</text>
"#,
            x + 5.0,
            pts as f64 / 1e9,
        ));
    }
    svg.push_str(
        r##"<text x="74" y="710" fill="#8b98a5" font-family="monospace" font-size="11">solid=visible | gap=not visible | dashed vertical=evidence anchor</text>
</svg>
"##,
    );
    std::fs::write(path, svg)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_panel<F, G>(
    svg: &mut String,
    samples: &[FrameSample],
    title: &str,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    max_value: f64,
    min_value: f64,
    x_at: F,
    value: G,
    color: &str,
) where
    F: Fn(u64) -> f64,
    G: Fn(&FrameSample) -> Option<f64>,
{
    svg.push_str(&format!(
        r##"<rect x="{left}" y="{top}" width="{width}" height="{height}" fill="#131c26" stroke="#263241"/>
<text x="{left}" y="{}" fill="#b8c4d0" font-family="monospace" font-size="13">{title}</text>
"##,
        top - 8.0
    ));
    for step in 0..=4 {
        let fraction = step as f64 / 4.0;
        let y = top + height - fraction * height;
        svg.push_str(&format!(
            r##"<line x1="{left}" y1="{y:.2}" x2="{:.2}" y2="{y:.2}" stroke="#263241" stroke-width="1"/>"##,
            left + width
        ));
    }
    let span = (max_value - min_value).abs().max(f64::EPSILON);
    let mut path = String::new();
    let mut drawing = false;
    for sample in samples {
        let Some(current) = value(sample).filter(|_| sample.visible) else {
            drawing = false;
            continue;
        };
        let x = x_at(sample.pts_ns);
        let y = top + height - ((current - min_value) / span).clamp(0.0, 1.0) * height;
        if drawing {
            path.push_str(&format!(" L {x:.2} {y:.2}"));
        } else {
            path.push_str(&format!("M {x:.2} {y:.2}"));
            drawing = true;
        }
    }
    svg.push_str(&format!(
        r#"<path d="{path}" fill="none" stroke="{color}" stroke-width="2"/>"#
    ));
    if let Some(last) = samples.last() {
        svg.push_str(&format!(
            r##"<text x="{:.2}" y="{}" fill="#8b98a5" font-family="monospace" font-size="11" text-anchor="end">{max_value:.1}</text>"##,
            left + width,
            top + 12.0
        ));
        svg.push_str(&format!(
            r##"<text x="{:.2}" y="{:.2}" fill="#8b98a5" font-family="monospace" font-size="11">{:.1}s</text>"##,
            left + width,
            top + height + 14.0,
            (last.pts_ns + last.duration_ns) as f64 / 1e9,
        ));
    }
    svg.push('\n');
}
