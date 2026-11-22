use anyhow::Context;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("fixtures/camera-extreme"));
    let profile = hdr_repair::fixture::generate_demo_fixture(&root)?;
    println!(
        "fixture={} frames={} video_sha256={}",
        root.display(),
        profile.frame_count,
        profile.source_video_sha256
    );
    let target = &profile.repeated_object.frames[1];
    println!(
        "acceptance frame={} pts_ns={} video_offset=0x{:X} metadata_offset=0x{:X}",
        target,
        profile.pts_ns(*target),
        profile.video_offset(*target),
        profile.metadata_offset(*target)
    );
    std::fs::canonicalize(root)
        .context("fixture was not created")
        .map(|_| ())
}
