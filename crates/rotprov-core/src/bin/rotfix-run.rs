use rotprov_core::model::{Conclusion, InputManifest};
use std::path::PathBuf;

/// headless 运行：读取 fixtures/manifest.json，执行管线，
/// 输出来源报告到 artifacts/provenance/report.json。
fn main() {
    let root = std::env::var("ROTFIX_ROOT").unwrap_or_else(|_| ".".to_string());
    let root = PathBuf::from(root);
    let manifest_path = root.join("fixtures/manifest.json");
    let text = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("无法读取 {}: {e}", manifest_path.display()));
    let manifests: Vec<InputManifest> =
        serde_json::from_str(&text).expect("manifest.json 格式错误");
    let manifests: Vec<InputManifest> = manifests
        .into_iter()
        .map(|mut m| {
            m.media_path = root.join(&m.media_path).to_string_lossy().to_string();
            m.sidecar_path = root.join(&m.sidecar_path).to_string_lossy().to_string();
            m
        })
        .collect();

    let report = rotprov_core::pipeline::run(&manifests);

    let out_dir = root.join("artifacts/provenance");
    std::fs::create_dir_all(&out_dir).expect("创建输出目录失败");
    let out_path = out_dir.join("report.json");
    std::fs::write(&out_path, serde_json::to_string_pretty(&report).unwrap())
        .expect("写入报告失败");

    println!("== 视频旋转元数据修复 · 来源检查 ==");
    for entry in &report.entries {
        let mark = match &entry.conclusion {
            Conclusion::Fixed { .. } => "[修复]",
            Conclusion::NoFixNeeded => "[跳过]",
            Conclusion::Undecidable { .. } => "[存疑]",
            Conclusion::Failed { .. } => "[异常]",
        };
        let trunc = if entry.record.truncated {
            format!(
                " 记录截断({}/{})",
                entry.record.steps.len(),
                entry.record.original_step_count
            )
        } else {
            String::new()
        };
        println!(
            "{mark} {} ({}): {} · 快照 {} 个 · 证据 {} 条{trunc}",
            entry.manifest.name,
            entry.manifest.id,
            entry.conclusion.label(),
            entry.snapshots.len(),
            entry.evidence.len(),
        );
    }
    println!(
        "临时工作区 {} → {}",
        report.temp_dir,
        if report.temp_cleaned {
            "已清理"
        } else {
            "未清理!"
        }
    );
    println!("报告已写入 {}", out_path.display());
}
