/// Format a nanosecond timestamp on the unified timeline as HH:MM:SS.mmm.
pub fn ns_to_timecode(ns: u64) -> String {
    let total_ms = ns / 1_000_000;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, ms)
}

/// Format a nanosecond duration as a compact human string like `1.500s`.
pub fn ns_to_duration(ns: u64) -> String {
    format!("{}.{:03}s", ns / 1_000_000_000, (ns / 1_000_000) % 1000)
}
