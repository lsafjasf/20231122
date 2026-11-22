use std::fs;
use std::io;
use std::path::Path;

use crate::model::{MetricLevel, ProcessingParams, WaveformPoint};

const MAX_RENDERED_POINTS: usize = 512;
const LANE_COUNT: usize = 3;
const LANE_HEIGHT: f64 = 132.0;
const TOP_MARGIN: f64 = 92.0;
const LEFT_MARGIN: f64 = 84.0;
const RIGHT_MARGIN: f64 = 32.0;
const BOTTOM_MARGIN: f64 = 52.0;

pub fn render_svg(
    points: &[WaveformPoint],
    params: &ProcessingParams,
    title: &str,
    width: f64,
    height: f64,
) -> String {
    let sampled = downsample(points, MAX_RENDERED_POINTS);
    let plot_width = (width - LEFT_MARGIN - RIGHT_MARGIN).max(1.0);
    let plot_height = (height - TOP_MARGIN - BOTTOM_MARGIN).max(1.0);
    let lane_gap = plot_height / LANE_COUNT as f64;
    let lane_height = lane_gap.clamp(36.0, LANE_HEIGHT);
    let mut svg = String::with_capacity(24_000);

    svg.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img" aria-label="{}">
<defs>
  <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#10141c"/><stop offset="1" stop-color="#080b10"/></linearGradient>
  <filter id="glow"><feGaussianBlur stdDeviation="2" result="blur"/><feMerge><feMergeNode in="blur"/><feMergeNode in="SourceGraphic"/></feMerge></filter>
</defs>
<rect width="100%" height="100%" fill="url(#bg)"/>
<text x="{LEFT_MARGIN}" y="38" fill="#e8eef8" font-family="monospace" font-size="20" font-weight="700">{}</text>
<text x="{LEFT_MARGIN}" y="62" fill="#7f8ba3" font-family="monospace" font-size="12">window_capacity={}  ema_alpha={:.3}  samples_rendered={}/{}  thresholds={}/{}/{}</text>
"##,
        escape_xml(title),
        escape_xml(title),
        params.window_capacity,
        params.ema_alpha,
        sampled.len(),
        points.len(),
        params.fps_warn,
        params.bitrate_warn_bps,
        params.drop_warn,
    ));

    let lanes = [
        LaneSpec {
            name: "FPS",
            color: "#43d9c5",
            value: |point| point.fps_normalized,
            warning: params.fps_warn / params.reference_fps.max(params.fps_warn),
            critical: params.fps_critical / params.reference_fps.max(params.fps_warn),
            label: format!(
                "warn <{:.2} fps / critical <{:.2}",
                params.fps_warn, params.fps_critical
            ),
        },
        LaneSpec {
            name: "BITRATE",
            color: "#f4b860",
            value: |point| point.bitrate_normalized,
            warning: params.bitrate_warn_bps as f64
                / params
                    .reference_bitrate_bps
                    .max(params.bitrate_critical_bps) as f64,
            critical: params.bitrate_critical_bps as f64
                / params
                    .reference_bitrate_bps
                    .max(params.bitrate_critical_bps) as f64,
            label: format!(
                "warn >{} kbps / critical >{} kbps",
                params.bitrate_warn_bps / 1_000,
                params.bitrate_critical_bps / 1_000
            ),
        },
        LaneSpec {
            name: "DROPPED",
            color: "#ff6d8c",
            value: |point| point.dropped_normalized,
            warning: params.drop_warn as f64 / params.drop_critical as f64,
            critical: 1.0,
            label: format!(
                "warn >={} / critical >={} frames",
                params.drop_warn, params.drop_critical
            ),
        },
    ];

    for (lane_index, lane) in lanes.iter().enumerate() {
        let lane_top = TOP_MARGIN + lane_index as f64 * lane_gap;
        let baseline = lane_top + lane_height;
        svg.push_str(&format!(
            r##"<g id="lane-{name}">
<rect x="{LEFT_MARGIN}" y="{lane_top:.2}" width="{plot_width:.2}" height="{lane_height:.2}" rx="6" fill="#0d121a" stroke="#222d3d"/>
<text x="20" y="{label_y:.2}" fill="{color}" font-family="monospace" font-size="13" font-weight="700">{name}</text>
<text x="20" y="{sub_y:.2}" fill="#66748c" font-family="monospace" font-size="9">{label}</text>
"##,
            name = lane.name,
            lane_top = lane_top,
            label_y = lane_top + 22.0,
            sub_y = lane_top + 37.0,
            plot_width = plot_width,
            lane_height = lane_height,
            color = lane.color,
            label = escape_xml(&lane.label),
        ));

        for value in [0.25_f64, 0.5, 0.75] {
            let y = value_y(baseline, lane_height, value);
            svg.push_str(&format!(
                r##"<line x1="{LEFT_MARGIN}" y1="{y:.2}" x2="{right:.2}" y2="{y:.2}" stroke="#182132" stroke-width="1"/>"##,
                y = y,
                right = LEFT_MARGIN + plot_width,
            ));
        }

        for (value, color) in [
            (lane.warning.clamp(0.0, 1.0), "#f4b860"),
            (lane.critical.clamp(0.0, 1.0), "#ff6d8c"),
        ] {
            let y = value_y(baseline, lane_height, value);
            svg.push_str(&format!(
                r##"<line x1="{LEFT_MARGIN}" y1="{y:.2}" x2="{right:.2}" y2="{y:.2}" stroke="{color}" stroke-opacity="0.5" stroke-dasharray="5 5" stroke-width="1"/>"##,
                y = y,
                right = LEFT_MARGIN + plot_width,
            ));
        }

        if sampled.is_empty() {
            continue;
        }

        let path = path_for(
            &sampled,
            lane,
            LEFT_MARGIN,
            plot_width,
            baseline,
            lane_height,
        );
        svg.push_str(&format!(
            r##"<path d="{path}" fill="none" stroke="{color}" stroke-width="2.2" stroke-linejoin="round" stroke-linecap="round" filter="url(#glow)"/>"##,
            color = lane.color,
        ));

        let marker_stride = (sampled.len() / 96).max(1);
        for (index, point) in sampled.iter().enumerate() {
            if index % marker_stride != 0 && index + 1 != sampled.len() {
                continue;
            }
            let x = x_for(index, sampled.len(), LEFT_MARGIN, plot_width);
            let y = value_y(baseline, lane_height, (lane.value)(point));
            let fill = level_color(point.level);
            svg.push_str(&format!(
                r##"<circle cx="{x:.2}" cy="{y:.2}" r="{}" fill="{fill}"><title>{}</title></circle>"##,
                if point.level == MetricLevel::Ok { 1.8 } else { 3.2 },
                escape_xml(&point_title(point)),
            ));
        }
    }

    let footer_y = height - 22.0;
    if let (Some(first), Some(last)) = (sampled.first(), sampled.last()) {
        svg.push_str(&format!(
            r##"<text x="{LEFT_MARGIN}" y="{footer_y}" fill="#66748c" font-family="monospace" font-size="10">t={:.3}ms track={} frame={}</text>
<text x="{right:.2}" y="{footer_y}" text-anchor="end" fill="#66748c" font-family="monospace" font-size="10">t={last_offset:.3}ms track={last_track} frame={last_frame}</text>"##,
            first.offset_ms,
            first.track,
            first.frame,
            last_offset = last.offset_ms,
            last_track = last.track,
            last_frame = last.frame,
            right = LEFT_MARGIN + plot_width,
        ));
    } else {
        svg.push_str(&format!(
            r##"<text x="{center:.2}" y="{center_y:.2}" text-anchor="middle" fill="#66748c" font-family="monospace" font-size="16">NO DATA - waiting for samples</text>"##,
            center = LEFT_MARGIN + plot_width / 2.0,
            center_y = TOP_MARGIN + plot_height / 2.0,
        ));
    }
    svg.push_str("</svg>\n");
    svg
}

pub fn write_svg(
    path: &Path,
    points: &[WaveformPoint],
    params: &ProcessingParams,
    title: &str,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, render_svg(points, params, title, 1200.0, 560.0))
}

struct LaneSpec<'a> {
    name: &'a str,
    color: &'a str,
    value: fn(&WaveformPoint) -> f64,
    warning: f64,
    critical: f64,
    label: String,
}

fn downsample(points: &[WaveformPoint], maximum: usize) -> Vec<&WaveformPoint> {
    if points.len() <= maximum {
        return points.iter().collect();
    }
    let stride = points.len().div_ceil(maximum);
    let mut sampled: Vec<_> = points.iter().step_by(stride).collect();
    if let Some(last) = points.last()
        && sampled
            .last()
            .is_none_or(|point| point.sequence != last.sequence)
    {
        sampled.push(last);
    }
    sampled
}

fn path_for(
    points: &[&WaveformPoint],
    lane: &LaneSpec<'_>,
    left: f64,
    width: f64,
    baseline: f64,
    height: f64,
) -> String {
    let mut path = String::with_capacity(points.len() * 20);
    for (index, point) in points.iter().enumerate() {
        let x = x_for(index, points.len(), left, width);
        let y = value_y(baseline, height, (lane.value)(point));
        if index == 0 {
            path.push_str(&format!("M{x:.2},{y:.2}"));
        } else {
            path.push_str(&format!("L{x:.2},{y:.2}"));
        }
    }
    path
}

fn x_for(index: usize, count: usize, left: f64, width: f64) -> f64 {
    if count <= 1 {
        left + width / 2.0
    } else {
        left + width * index as f64 / (count - 1) as f64
    }
}

fn value_y(baseline: f64, height: f64, normalized: f64) -> f64 {
    baseline - normalized.clamp(0.0, 1.0) * (height - 8.0)
}

fn level_color(level: MetricLevel) -> &'static str {
    match level {
        MetricLevel::Ok => "#43d9c5",
        MetricLevel::Warning => "#f4b860",
        MetricLevel::Critical => "#ff475f",
    }
}

fn point_title(point: &WaveformPoint) -> String {
    format!(
        "t={:.3}ms track={} frame={} fps={:.4} bitrate={}bps dropped_delta={} level={:?}",
        point.offset_ms,
        point.track,
        point.frame,
        point.fps,
        point.bitrate_bps,
        point.dropped_delta,
        point.level
    )
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
