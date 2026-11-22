//! Text rendering of an integrity report, shared by CLI and GUI.

use crate::isobmff::{ErrorKind, Report};
use crate::repair::repair_advice;

pub fn kind_label(kind: Option<ErrorKind>) -> &'static str {
    match kind {
        None => "OK",
        Some(ErrorKind::UnknownType) => "UNKNOWN",
        Some(ErrorKind::Truncated) => "TRUNCATED",
        Some(ErrorKind::InvalidSize) => "INVALID-SIZE",
        Some(ErrorKind::Malformed) => "MALFORMED",
    }
}

pub fn render_report_table(report: &Report) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:>10}  {:<8}  {:>10}  {:<18}  {:<9}  {}\n",
        "offset", "type", "size", "owner", "status", "repairability"
    ));
    out.push_str(&"-".repeat(92));
    out.push('\n');
    for b in &report.boxes {
        let indent = "  ".repeat(b.depth);
        out.push_str(&format!(
            "{:>10}  {}{:<7}  {:>10}  {:<18}  {:<9}  {}\n",
            b.offset,
            indent,
            b.box_type.to_string(),
            b.declared_size,
            if b.owner.is_empty() {
                "file".to_string()
            } else {
                b.owner.clone()
            },
            kind_label(b.error),
            b.repair.label()
        ));
    }
    out
}

pub fn render_summary(report: &Report, path: &std::path::Path) -> String {
    let mut out = String::new();
    out.push_str(&format!("file: {}\n", path.display()));
    out.push_str(&format!("file size: {} bytes\n", report.file_size));
    out.push_str(&format!("boxes found: {}\n", report.boxes.len()));
    out.push_str(&format!(
        "tracks in headers (tkhd): {}\n",
        report.track_ids.len()
    ));
    match report.first_error_offset() {
        Some(off) => out.push_str(&format!("first error offset: {} (0x{:x})\n", off, off)),
        None => out.push_str("first error offset: none\n"),
    }
    out.push_str(&format!("structural errors: {}\n", report.errors.len()));
    let unknown: Vec<_> = report.unknown_boxes().collect();
    out.push_str(&format!("unknown boxes: {}\n", unknown.len()));
    out
}

pub fn render_errors(report: &Report) -> String {
    let mut out = String::new();
    if report.errors.is_empty() {
        out.push_str("no structural errors detected\n");
        return out;
    }
    for e in &report.errors {
        out.push_str(&format!(
            "- offset {} [{}]: {}\n",
            e.offset,
            e.box_type
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".into()),
            e.message
        ));
    }
    let advice = repair_advice(report);
    if !advice.is_empty() {
        out.push_str("\nrepair suggestions:\n");
        for line in advice {
            out.push_str(&format!("  * {}\n", line));
        }
    }
    out
}
