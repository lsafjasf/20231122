use crate::experiment::{ScenarioConfig, ASSET_A, ASSET_B, CONTROL};
use crate::hash;
use crate::model::Check;
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};

const FAILURE_ACTIONS: &[(&str, &str)] = &[
    (
        "recovery-restores-valid-proxy",
        "source scripts/env.sh && GST_DEBUG=3 cargo run -- run --out /tmp/proxyrecover-debug 2>/tmp/proxyrecover-gst.log; \
         gst-inspect-1.0 pngdec pngenc videoscale; \
         cat artifacts/latest/evidence/meta-clip-a-recover-clip-a.json",
    ),
    (
        "post-recovery-replay-no-regeneration",
        "cargo run -- replay artifacts/latest/journal.jsonl; \
         rg 'post-recovery-replay|generation' artifacts/latest/journal.jsonl; \
         sha256sum artifacts/latest/evidence/proxy-clip-a-*.png",
    ),
    (
        "unknown-extension-control-quarantined",
        "rg 'unsupported_extension|unknown-extension-control' artifacts/latest/journal.jsonl; \
         find artifacts/latest/workspace/proxies -maxdepth 1 -type f -printf '%f\\n'; \
         test ! -e artifacts/latest/workspace/proxies/control-unknown.proxy.png",
    ),
    (
        "recovery-order-independent",
        "cargo run -- run --out /tmp/proxyrecover-order; \
         sha256sum /tmp/proxyrecover-order/evidence/proxy-clip-*-recover-clip-*-reversed.png",
    ),
    (
        "duplicate-submission-idempotent",
        "rg 'repeat-submit-clip-a|verify-repeat' artifacts/latest/journal.jsonl; \
         stat -c '%n %Y %s' artifacts/latest/workspace/proxies/clip-a.proxy.png; \
         cargo run -- replay artifacts/latest/journal.jsonl",
    ),
    (
        "journal-and-key-evidence-retained",
        "cargo run -- replay artifacts/latest/journal.jsonl; \
         cargo run -- verify-artifacts artifacts/latest/evidence/manifest.json; \
         wc -l artifacts/latest/journal.jsonl",
    ),
];

pub fn render_report(
    out_dir: &Path,
    journal_path: &Path,
    config: &ScenarioConfig,
    checks: &[Check],
    all_passed: bool,
) -> String {
    let failed: Vec<&Check> = checks.iter().filter(|check| !check.passed).collect();
    let mut text = String::new();
    text.push_str("# 素材代理失效恢复实验报告\n\n");
    text.push_str("## 实验概述\n\n");
    text.push_str("- 技术栈: Rust + GStreamer + GTK4\n");
    text.push_str(&format!("- 输出目录: `{}`\n", out_dir.display()));
    text.push_str(&format!("- 可回放日志: `{}`\n", journal_path.display()));
    text.push_str(&format!(
        "- 源素材: `{}` / `{}`，{}x{}；代理: {}x{} {}\n",
        config.pattern_a,
        config.pattern_b,
        config.source_width,
        config.source_height,
        config.proxy_width,
        config.proxy_height,
        config.proxy_spec().encoder
    ));
    text.push_str(&format!(
        "- 资产: `{ASSET_A}`、`{ASSET_B}`；未知扩展对照: `{CONTROL}.{}`\n",
        config.unknown_extension
    ));
    text.push_str(
        "- 故障注入: 保留元数据，替换代理前缀并删除原始前 24 字节，使 SHA-256 与解码均失效\n",
    );
    text.push_str(
        "- 恢复事务: 校验源哈希 → 临时文件重建 → 解码指纹校验 → 原子重命名 → 更新元数据\n",
    );
    text.push_str(
        "- 恢复后重放: 对 `clip-a` 使用同一输入重放一次；另做 A→B 与 B→A 顺序对照、两次重复提交\n\n",
    );

    text.push_str("## 自动结论\n\n");
    text.push_str(&format!(
        "总体: **{}**（自动检查 {} 项，通过 {} 项）\n\n",
        if all_passed {
            "全部通过"
        } else {
            "存在失败项"
        },
        checks.len(),
        checks.iter().filter(|check| check.passed).count()
    ));
    text.push_str("| 检查项 | 结果 | 说明 |\n| --- | --- | --- |\n");
    for check in checks {
        text.push_str(&format!(
            "| `{}` | {} | {} |\n",
            check.name,
            if check.passed { "PASS" } else { "FAIL" },
            check.detail.replace('|', "\\|")
        ));
    }
    text.push_str(
        "\n自动结论仅覆盖可计算约束：文件哈希、GStreamer 解码能力、尺寸、元数据状态、\
         generation 是否变化、文件是否被改写，以及未知扩展是否被拒绝。\
         它不替代内容正确性、产品策略或真实素材兼容性的人工判断。\n\n",
    );

    text.push_str("## 人工确认\n\n");
    text.push_str("以下项目不由程序自动判定，请逐项核对并填写结果：\n\n");
    text.push_str("- [ ] 打开 `evidence/proxy-clip-a-initial.png` 与 `evidence/proxy-clip-a-recover-clip-a.png`，确认降采样内容正常、无错帧/错素材\n");
    text.push_str("- [ ] 打开 `evidence/proxy-clip-a-invalid-initial.bin`，确认该文件确实已被破坏且不能作为代理使用\n");
    text.push_str("- [ ] 对照 `evidence/meta-clip-a-recover-clip-a.json` 与 `evidence/meta-clip-b-recover-clip-b.json`，确认 generation、SHA-256 与预期业务语义一致\n");
    text.push_str("- [ ] 在真实产品中确认未知扩展被隔离是可接受策略，而不是应回退到内容探测\n");
    text.push_str(
        "- [ ] 运行 `cargo run -- gui`，确认界面中的“自动结论”和“人工确认”分区清晰、回放按钮可用\n",
    );
    text.push_str("- [ ] 抽查报告失败处置命令可在当前环境直接执行\n");
    text.push_str("- 确认人: __________  日期: __________  结论: [ ] 接受 / [ ] 拒绝\n\n");

    text.push_str("## 失败时的可执行后续操作\n\n");
    if failed.is_empty() {
        text.push_str("本次没有失败项。以下命令保留用于回归失败时的定位。\n\n");
    } else {
        text.push_str("本次失败项及建议操作：\n\n");
        for check in &failed {
            text.push_str(&format!("### `{}`\n\n", check.name));
            if let Some((_, command)) = FAILURE_ACTIONS.iter().find(|(name, _)| *name == check.name)
            {
                text.push_str("```bash\n");
                text.push_str(command);
                text.push_str("\n```\n\n");
            }
            text.push_str(&format!("观察项：{}\n\n", check.detail));
        }
    }
    text.push_str("| 检查项 | 可执行操作 |\n| --- | --- |\n");
    for (name, command) in FAILURE_ACTIONS {
        text.push_str(&format!(
            "| `{name}` | `{}` |\n",
            command.replace('|', "\\|")
        ));
    }
    text.push_str(
        "\n通用排查：先运行 `cargo test`，再运行 `cargo run -- replay artifacts/latest/journal.jsonl`；\
         两者都失败时删除 `artifacts/latest/replay` 后重试，并用 `GST_DEBUG=3` 保存 GStreamer 日志。\n\n",
    );
    text.push_str("## 回放方法\n\n");
    text.push_str("```bash\ncargo run -- replay artifacts/latest/journal.jsonl\n```\n\n");
    text.push_str(
        "回放会在 `artifacts/latest/replay/` 重建场景，并逐条比较步骤的输入、状态、结果；\
         路径与时间戳不参与比较，实验语义必须一致。\n",
    );
    text
}

pub fn render_failure_report(out_dir: &Path, config: &ScenarioConfig, error: &str) -> String {
    let mut text = String::new();
    text.push_str("# 素材代理失效恢复实验报告（运行失败）\n\n");
    text.push_str("## 自动结论\n\n");
    text.push_str(&format!("总体: **运行中断**\n\n错误：`{error}`\n\n"));
    text.push_str(&format!(
        "- 输出目录: `{}`\n- 代理规格: {}x{} `{}`\n\n",
        out_dir.display(),
        config.proxy_width,
        config.proxy_height,
        config.proxy_spec().encoder
    ));
    text.push_str("## 人工确认\n\n");
    text.push_str("- [ ] 保留当前输出目录，不要删除现场\n");
    text.push_str("- [ ] 根据下方命令复现并记录 GStreamer 错误\n");
    text.push_str("- [ ] 确认人: __________  日期: __________\n\n");
    text.push_str("## 失败时的可执行后续操作\n\n");
    text.push_str("```bash\nsource scripts/env.sh\nGST_DEBUG=3 cargo run -- run 2>/tmp/proxyrecover-gst.log\ngst-inspect-1.0 pngdec pngenc videoscale\ncargo test\n```\n\n");
    text.push_str(
        "若错误包含 `ElementFactory` 或 `MISSING`，检查 `GST_PLUGIN_PATH`；\
        若包含 `源素材在恢复前已变化`，从新的输出目录重跑，避免复用被修改的 workspace。\n",
    );
    text
}

pub fn render_replay_report(
    original: &Path,
    replay_dir: &Path,
    matched: usize,
    total: usize,
    mismatches: &[String],
) -> String {
    let mut text = String::new();
    text.push_str("# 回放验证报告\n\n");
    text.push_str(&format!("- 原始日志: `{}`\n", original.display()));
    text.push_str(&format!("- 回放目录: `{}`\n", replay_dir.display()));
    text.push_str(&format!("- 逐条语义匹配: **{matched}/{total}**\n\n",));
    text.push_str("## 自动结论\n\n");
    if mismatches.is_empty() {
        text.push_str("- [PASS] 所有步骤的输入、前状态、后状态和结果均与原始记录一致\n\n");
    } else {
        text.push_str("- [FAIL] 检测到回放差异：\n\n");
        for mismatch in mismatches {
            text.push_str(&format!("  - {mismatch}\n"));
        }
        text.push('\n');
    }
    text.push_str("## 人工确认\n\n");
    text.push_str("- [ ] 确认回放目录中的代理图片可打开且内容正确\n");
    text.push_str("- [ ] 确认未知扩展对照未产生代理文件\n");
    text.push_str("- [ ] 确认人: __________  日期: __________\n\n");
    text.push_str("## 失败时的可执行后续操作\n\n");
    text.push_str("```bash\ncargo run -- replay artifacts/latest/journal.jsonl\nrg 'MISMATCH|mismatch' artifacts/latest/replay/REPLAY.md\njq -c '{seq,step,state_before,state_after,result}' artifacts/latest/journal.jsonl > /tmp/original.steps\njq -c '{seq,step,state_before,state_after,result}' artifacts/latest/replay/journal.jsonl > /tmp/replay.steps\ndiff -u /tmp/original.steps /tmp/replay.steps\n```\n");
    text
}

#[derive(Debug, Serialize)]
struct ManifestFile {
    path: String,
    size: u64,
    sha256: String,
}

pub fn evidence_manifest(
    out_dir: &Path,
    manifest_path: &Path,
) -> Result<serde_json::Value, String> {
    let mut files = Vec::new();
    let mut stack = vec![out_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("读取 {}: {e}", dir.display()))?
        {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|e| e.to_string())?;
            if file_type.is_dir() {
                if path.file_name().and_then(|s| s.to_str()) != Some("replay") {
                    stack.push(path);
                }
                continue;
            }
            if !file_type.is_file() || path == manifest_path {
                continue;
            }
            files.push(ManifestFile {
                path: hash::relative_string(out_dir, &path),
                size: entry.metadata().map_err(|e| e.to_string())?.len(),
                sha256: hash::sha256_file(&path)?,
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(json!({
        "schema_version": 1,
        "algorithm": "sha256",
        "file_count": files.len(),
        "files": files,
    }))
}

pub fn verify_manifest(manifest_path: &Path) -> Result<usize, String> {
    let text = std::fs::read_to_string(manifest_path)
        .map_err(|e| format!("读取清单 {}: {e}", manifest_path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("解析清单: {e}"))?;
    let base = manifest_path
        .parent()
        .and_then(Path::parent)
        .ok_or("清单路径没有父目录")?;
    let files = value["files"].as_array().ok_or("清单缺少 files 数组")?;
    for file in files {
        let rel = file["path"].as_str().ok_or("清单项缺少 path")?;
        let expected = file["sha256"].as_str().ok_or("清单项缺少 sha256")?;
        let path = base.join(rel);
        let actual = hash::sha256_file(&path)?;
        if actual != expected {
            return Err(format!(
                "证据校验失败: {} expected={} actual={actual}",
                path.display(),
                expected
            ));
        }
    }
    Ok(files.len())
}

pub fn manifest_files(manifest_path: &Path) -> Result<Vec<PathBuf>, String> {
    let text = std::fs::read_to_string(manifest_path).map_err(|e| e.to_string())?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let base = manifest_path
        .parent()
        .and_then(Path::parent)
        .ok_or("清单路径没有父目录")?;
    Ok(value["files"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|file| file["path"].as_str().map(|rel| base.join(rel)))
        .collect())
}
