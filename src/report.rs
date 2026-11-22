//! 修复前后对照清单导出（Markdown / CSV）。

use crate::fix::RepairSession;
use std::fmt::Write as _;

fn opt(v: &Option<crate::meta::FieldValue>) -> String {
    v.map(|x| x.to_string())
        .unwrap_or_else(|| "（缺失）".to_string())
}

/// 生成 Markdown 对照表：帧号 | 字段 | 修复前 | 修复后 | 修复依据。
pub fn to_markdown(session: &RepairSession) -> String {
    let mut out = String::new();
    out.push_str("# HDR 元数据修复对照清单\n\n");
    let _ = writeln!(out, "- 帧数：{}", session.frames.len());
    let _ = writeln!(out, "- 已应用修改：{} 处", session.edits().len());
    let _ = writeln!(out, "- 剩余未解决问题：{} 个\n", session.remaining_issues());
    out.push_str("| 帧号 | 字段 | 修复前 | 修复后 | 修复依据 |\n");
    out.push_str("|---:|---|---|---|---|\n");
    for e in session.edits() {
        let frame_no = session.frames[e.frame_idx].frame_no;
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            frame_no,
            e.field.label(),
            opt(&e.before),
            e.after,
            e.rationale
        );
    }
    if session.edits().is_empty() {
        out.push_str("| - | - | - | - | 未做任何修改 |\n");
    }
    out
}

/// 生成 CSV 对照表（同列，便于表格软件打开）。
pub fn to_csv(session: &RepairSession) -> String {
    let mut out = String::from("帧号,字段,修复前,修复后,修复依据\n");
    for e in session.edits() {
        let frame_no = session.frames[e.frame_idx].frame_no;
        let row = format!(
            "{},{},{},{},{}\n",
            frame_no,
            e.field.label(),
            csv_escape(&opt(&e.before)),
            csv_escape(&e.after.to_string()),
            csv_escape(&e.rationale)
        );
        out.push_str(&row);
    }
    out
}

fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
