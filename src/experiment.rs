use crate::analysis;
use crate::journal::{read_journal, Journal};
use crate::pipeline::{self, ExportSpec};
use serde_json::json;
use std::path::{Path, PathBuf};

const RATE: u32 = 48_000;
const CHANNELS: u32 = 1;
const DURATION_SECS: f64 = 2.0;
const CANCEL_AT_NS: u64 = 1_000_000_000; // abort mid-export at 1.0 s of 2.0 s
const CANCEL_BYTES_TOLERANCE: u64 = 64 * 1024;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Check {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

pub struct RunOutput {
    pub journal_path: PathBuf,
    pub report_path: PathBuf,
    pub checks: Vec<Check>,
}

fn spec(input: &Path, output: &Path, dithering: &str, ns: &str, cancel: bool) -> ExportSpec {
    ExportSpec {
        input_raw: input.to_path_buf(),
        output_wav: output.to_path_buf(),
        rate: RATE,
        channels: CHANNELS,
        dithering: dithering.into(),
        noise_shaping: ns.into(),
        cancel_at_ns: cancel.then_some(CANCEL_AT_NS),
    }
}

fn export_step(
    j: &mut Journal,
    purpose: &str,
    name: &str,
    spec: &ExportSpec,
) -> Result<pipeline::ExportOutcome, String> {
    let before = "pipeline=NULL".to_string();
    let outcome = pipeline::export(spec)?;
    j.append(
        name,
        purpose,
        json!({
            "input_raw": spec.input_raw,
            "output_wav": spec.output_wav,
            "rate": spec.rate,
            "channels": spec.channels,
            "dithering": spec.dithering,
            "noise_shaping": spec.noise_shaping,
            "cancel_at_ns": spec.cancel_at_ns,
        }),
        &before,
        &format!("pipeline={}", outcome.final_state),
        serde_json::to_value(&outcome).map_err(|e| e.to_string())?,
        vec![spec.output_wav.to_string_lossy().into_owned()],
    )
    .map_err(|e| e.to_string())?;
    Ok(outcome)
}

pub fn run_experiment(out_dir: &Path, progress: &mut dyn FnMut(&str)) -> Result<RunOutput, String> {
    pipeline::init()?;
    let ev = out_dir.join("evidence");
    std::fs::create_dir_all(&ev).map_err(|e| e.to_string())?;
    let mut j = Journal::create(&out_dir.join("journal.jsonl")).map_err(|e| e.to_string())?;

    progress("recording GStreamer environment");
    let env_path = pipeline::record_gst_env(&ev)?;
    j.append(
        "gst-env",
        "capture toolchain/plugin environment as evidence",
        json!({}),
        "n/a",
        "recorded",
        json!({"path": env_path}),
        vec![env_path.to_string_lossy().into_owned()],
    )
    .map_err(|e| e.to_string())?;

    progress("step: generate-source");
    let src = ev.join("source.f32le");
    let samples = analysis::generate_source(&src, RATE, DURATION_SECS)?;
    let src_sha = analysis::sha256_file(&src)?;
    j.append(
        "generate-source",
        "deterministic 440 Hz f32le tone, input to all exports",
        json!({"rate": RATE, "secs": DURATION_SECS, "freq": 440.0, "amp": 0.5}),
        "no source file",
        "source written",
        json!({"samples": samples, "sha256": src_sha}),
        vec![src.to_string_lossy().into_owned()],
    )
    .map_err(|e| e.to_string())?;

    progress("step: export-full-dither");
    let dither = export_step(
        &mut j,
        "full export with TPDF dither + medium noise shaping",
        "export-full-dither",
        &spec(&src, &ev.join("full-dither.wav"), "tpdf", "medium", false),
    )?;

    progress("step: export-full-nodither");
    let nodither = export_step(
        &mut j,
        "full export with dithering disabled (analysis contrast)",
        "export-full-nodither",
        &spec(&src, &ev.join("full-nodither.wav"), "none", "none", false),
    )?;

    progress("step: export-full-tpdf");
    let _tpdf = export_step(
        &mut j,
        "full export with TPDF dither only (isolates dither from noise shaping)",
        "export-full-tpdf",
        &spec(&src, &ev.join("full-tpdf.wav"), "tpdf", "none", false),
    )?;

    progress("step: export-cancel-dither (mid-way cancel, test)");
    let cancel_test = export_step(
        &mut j,
        "TEST: abort dithered export mid-way at 1.0 s (no EOS)",
        "export-cancel-dither",
        &spec(&src, &ev.join("cancel-dither.wav"), "tpdf", "medium", true),
    )?;

    progress("step: export-cancel-control (mid-way cancel, control)");
    let cancel_ctrl = export_step(
        &mut j,
        "CONTROL: identical mid-way cancel with dithering disabled",
        "export-cancel-control",
        &spec(&src, &ev.join("cancel-control.wav"), "none", "none", true),
    )?;

    progress("step: verify-order (swapped submission order)");
    let rev_nodither = pipeline::export(&spec(
        &src,
        &ev.join("order-nodither.wav"),
        "none",
        "none",
        false,
    ))?;
    let rev_dither = pipeline::export(&spec(
        &src,
        &ev.join("order-dither.wav"),
        "tpdf",
        "medium",
        false,
    ))?;
    let order_ok = rev_dither.sha256 == dither.sha256 && rev_nodither.sha256 == nodither.sha256;
    j.append(
        "verify-order",
        "re-submit exports in reversed order; results must be identical",
        json!({"order": ["nodither", "dither"]}),
        "two prior full exports recorded",
        "reversed re-submission complete",
        json!({
            "dither_sha256": rev_dither.sha256,
            "nodither_sha256": rev_nodither.sha256,
            "matches_original": order_ok,
        }),
        vec![
            ev.join("order-dither.wav").to_string_lossy().into_owned(),
            ev.join("order-nodither.wav").to_string_lossy().into_owned(),
        ],
    )
    .map_err(|e| e.to_string())?;

    progress("step: verify-repeat (repeated identical submissions)");
    let rep1 = pipeline::export(&spec(
        &src,
        &ev.join("repeat-1.wav"),
        "tpdf",
        "medium",
        false,
    ))?;
    let rep2 = pipeline::export(&spec(
        &src,
        &ev.join("repeat-2.wav"),
        "tpdf",
        "medium",
        false,
    ))?;
    let repeat_ok = rep1.sha256 == dither.sha256 && rep2.sha256 == dither.sha256;
    j.append(
        "verify-repeat",
        "submit the identical dithered export twice more; hashes must match",
        json!({"repetitions": 2}),
        "one prior dithered export recorded",
        "repeated submissions complete",
        json!({
            "repeat1_sha256": rep1.sha256,
            "repeat2_sha256": rep2.sha256,
            "matches_original": repeat_ok,
        }),
        vec![
            ev.join("repeat-1.wav").to_string_lossy().into_owned(),
            ev.join("repeat-2.wav").to_string_lossy().into_owned(),
        ],
    )
    .map_err(|e| e.to_string())?;

    progress("step: analyze");
    let source = analysis::read_f32le(&src)?;
    let m_dither = analysis::error_metrics(&source, &analysis::read_wav_s16(&ev.join("full-dither.wav"))?);
    let m_nodither =
        analysis::error_metrics(&source, &analysis::read_wav_s16(&ev.join("full-nodither.wav"))?);
    let m_tpdf = analysis::error_metrics(&source, &analysis::read_wav_s16(&ev.join("full-tpdf.wav"))?);

    let mut checks = Vec::new();
    let ratio = m_tpdf.err_rms_lsb / m_nodither.err_rms_lsb.max(1e-9);
    checks.push(Check {
        name: "dither-applied".into(),
        passed: (1.4..=2.0).contains(&ratio)
            && (0.4..=0.6).contains(&m_tpdf.err_rms_lsb)
            && m_dither.err_rms_lsb > m_tpdf.err_rms_lsb,
        detail: format!(
            "err_rms nodither={:.4} LSB (theory q/sqrt(12)={:.3}), tpdf={:.4} LSB \
             (TPDF dither+quantization theory q/2=0.5), tpdf+ns-medium={:.4} LSB; \
             ratio tpdf/nodither={ratio:.3} (theory ~1.73); \
             noise shaping raises total RMS as expected",
            m_nodither.err_rms_lsb,
            1.0 / 12.0f64.sqrt(),
            m_tpdf.err_rms_lsb,
            m_dither.err_rms_lsb,
        ),
    });
    checks.push(Check {
        name: "repeat-submission-deterministic".into(),
        passed: repeat_ok,
        detail: format!("original={} repeat1={} repeat2={}", dither.sha256, rep1.sha256, rep2.sha256),
    });
    checks.push(Check {
        name: "order-independent".into(),
        passed: order_ok,
        detail: format!(
            "dither {} vs {}, nodither {} vs {}",
            dither.sha256, rev_dither.sha256, nodither.sha256, rev_nodither.sha256
        ),
    });
    let cancel_bytes_close =
        cancel_test.bytes_written.abs_diff(cancel_ctrl.bytes_written) <= CANCEL_BYTES_TOLERANCE;
    checks.push(Check {
        name: "midway-cancel-partial-output".into(),
        passed: cancel_test.cancelled
            && cancel_ctrl.cancelled
            && !cancel_test.header_sane
            && !cancel_ctrl.header_sane
            && cancel_test.bytes_written > 44
            && cancel_test.bytes_written < dither.bytes_written
            && cancel_bytes_close,
        detail: format!(
            "test: cancelled={} bytes={} header_sane={} pos={:?}; control: cancelled={} bytes={} \
             header_sane={} pos={:?}; full={} bytes",
            cancel_test.cancelled, cancel_test.bytes_written, cancel_test.header_sane,
            cancel_test.position_at_end_ns, cancel_ctrl.cancelled, cancel_ctrl.bytes_written,
            cancel_ctrl.header_sane, cancel_ctrl.position_at_end_ns, dither.bytes_written
        ),
    });
    checks.push(Check {
        name: "state-recovery-after-cancel".into(),
        passed: cancel_test.final_state == "Null"
            && cancel_ctrl.final_state == "Null"
            && repeat_ok,
        detail: format!(
            "post-cancel states: test={}, control={}; subsequent exports still deterministic",
            cancel_test.final_state, cancel_ctrl.final_state
        ),
    });

    j.append(
        "analyze",
        "compute error metrics and automated checks",
        json!({"checks": ["dither-applied","repeat-submission-deterministic","order-independent","midway-cancel-partial-output","state-recovery-after-cancel"]}),
        "all exports recorded",
        "analysis complete",
        json!({
            "metrics_dither": m_dither,
            "metrics_tpdf": m_tpdf,
            "metrics_nodither": m_nodither,
            "checks": checks,
        }),
        vec![],
    )
    .map_err(|e| e.to_string())?;

    progress("step: report");
    let report_path = out_dir.join("REPORT.md");
    let report = render_report(out_dir, j.path(), &checks, &m_dither, &m_nodither);
    std::fs::write(&report_path, report).map_err(|e| e.to_string())?;
    j.append(
        "report",
        "render REPORT.md separating automated conclusions from manual confirmation",
        json!({}),
        "analysis complete",
        "report written",
        json!({"path": report_path}),
        vec![report_path.to_string_lossy().into_owned()],
    )
    .map_err(|e| e.to_string())?;

    Ok(RunOutput {
        journal_path: j.path().to_path_buf(),
        report_path,
        checks,
    })
}

pub fn run_summary(out: &RunOutput) -> String {
    let mut s = format!(
        "journal: {}\nreport:  {}\nchecks:\n",
        out.journal_path.display(),
        out.report_path.display()
    );
    for c in &out.checks {
        s.push_str(&format!("  [{}] {} - {}\n", if c.passed { "PASS" } else { "FAIL" }, c.name, c.detail));
    }
    s
}

fn render_report(
    out_dir: &Path,
    journal_path: &Path,
    checks: &[Check],
    m_dither: &analysis::ErrorMetrics,
    m_nodither: &analysis::ErrorMetrics,
) -> String {
    let all_pass = checks.iter().all(|c| c.passed);
    let mut r = String::new();
    r.push_str("# 音频导出抖动诊断报告\n\n");
    r.push_str("## 实验概述\n\n");
    r.push_str(&format!("- 输出目录: `{}`\n", out_dir.display()));
    r.push_str(&format!("- 步骤日志(可回放): `{}`\n", journal_path.display()));
    r.push_str("- 技术栈: Rust + GStreamer(audioconvert/wavenc) + GTK4\n");
    r.push_str(&format!(
        "- 信号链: 440 Hz f32le 单声道 {} Hz -> audioconvert -> S16LE WAV\n",
        RATE
    ));
    r.push_str("- 中途取消: 在 1.0 s / 2.0 s 处将管线直接置为 NULL(不发送 EOS),测试组启用抖动,对照组禁用抖动\n\n");

    r.push_str("## 自动结论\n\n");
    r.push_str(&format!(
        "总体: **{}**\n\n",
        if all_pass { "全部通过" } else { "存在失败项" }
    ));
    for c in checks {
        r.push_str(&format!(
            "- [{}] **{}**: {}\n",
            if c.passed { "PASS" } else { "FAIL" },
            c.name,
            c.detail
        ));
    }
    r.push_str(&format!(
        "\n量化误差指标: TPDF+噪声整形 RMS={:.4} LSB(峰值 {:.2}),无抖动 RMS={:.4} LSB(峰值 {:.2})\n\n",
        m_dither.err_rms_lsb, m_dither.err_peak_lsb, m_nodither.err_rms_lsb,
        m_nodither.err_peak_lsb
    ));

    r.push_str("## 人工确认\n\n");
    r.push_str("以下项目无法自动判定,需人工核对后勾选:\n\n");
    r.push_str("- [ ] 试听 `evidence/full-dither.wav` 与 `evidence/full-nodither.wav`,确认无可闻量化失真异常\n");
    r.push_str("- [ ] 查看 `evidence/cancel-dither.wav` 的部分文件大小与取消位置符合预期(约 50%)\n");
    r.push_str("- [ ] 在 GTK4 界面中手动触发一次“中途取消”,确认界面状态与日志一致\n");
    r.push_str("- [ ] 核对 `evidence/gst-env.txt` 中的插件版本与发布环境一致\n");
    r.push_str("- [ ] 确认人: __________  日期: __________\n\n");

    r.push_str("## 失败时的可执行后续操作\n\n");
    if all_pass {
        r.push_str("本次无失败项。若后续回归出现失败,按下表处理。\n\n");
    }
    r.push_str("| 失败检查项 | 后续操作 |\n| --- | --- |\n");
    r.push_str("| dither-applied | `GST_DEBUG=audioconvert:5 cargo run -- run` 确认 dithering/noise-shaping 属性生效;检查 `evidence/gst-env.txt` 中 audioconvert 是否缺失 |\n");
    r.push_str("| repeat-submission-deterministic | 检查是否有并发写入同一输出;用 `sha256sum evidence/repeat-*.wav` 复核;确认源文件 `evidence/source.f32le` 哈希未变 |\n");
    r.push_str("| order-independent | 检查全局状态污染(GST_REGISTRY 缓存);删除 `$GST_REGISTRY` 后重跑 `cargo run -- run` |\n");
    r.push_str("| midway-cancel-partial-output | 提高轮询粒度或调整 `CANCEL_AT_NS`;用 `gst-launch-1.0` 手工复现取消;确认 filesink 未缓冲(`gst-inspect-1.0 filesink` 查看 buffer-mode) |\n");
    r.push_str("| state-recovery-after-cancel | 检查管线是否在取消后泄漏 PLAYING 状态;在 GUI 中复现并导出 `GST_DEBUG=3` 日志 |\n\n");
    r.push_str("回放验证: `cargo run -- replay <journal.jsonl>`(取消步骤按结构属性比对,其余按 SHA-256 精确比对)\n");
    r
}

/// Re-execute a journal and verify recorded results.
/// Cancel steps are timing-sensitive, so they are compared structurally
/// (cancelled flag, header sanity, byte count within tolerance); all other
/// artifacts are compared by exact SHA-256.
pub fn replay_journal(journal_path: &Path) -> Result<bool, String> {
    pipeline::init()?;
    let records = read_journal(journal_path).map_err(|e| e.to_string())?;
    if records.is_empty() {
        return Err("journal is empty".into());
    }
    let replay_dir = journal_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("replay");
    std::fs::create_dir_all(&replay_dir).map_err(|e| e.to_string())?;

    let mut ok = true;
    let mut src: Option<PathBuf> = None;
    for rec in &records {
        match rec.step.as_str() {
            "generate-source" => {
                let out = replay_dir.join("source.f32le");
                let rate = rec.input["rate"].as_u64().unwrap_or(RATE as u64) as u32;
                let secs = rec.input["secs"].as_f64().unwrap_or(DURATION_SECS);
                analysis::generate_source(&out, rate, secs)?;
                let sha = analysis::sha256_file(&out)?;
                let want = rec.result["sha256"].as_str().unwrap_or("");
                let m = sha == want;
                println!("[replay #{:02}] generate-source: {}", rec.seq, mark(m));
                ok &= m;
                src = Some(out);
            }
            name if name.starts_with("export-") => {
                let src = src.clone().ok_or("journal order: export before generate-source")?;
                let out = replay_dir.join(format!("{name}.wav"));
                let s = ExportSpec {
                    input_raw: src,
                    output_wav: out.clone(),
                    rate: rec.input["rate"].as_u64().unwrap_or(RATE as u64) as u32,
                    channels: rec.input["channels"].as_u64().unwrap_or(1) as u32,
                    dithering: rec.input["dithering"].as_str().unwrap_or("none").into(),
                    noise_shaping: rec.input["noise_shaping"].as_str().unwrap_or("none").into(),
                    cancel_at_ns: rec.input["cancel_at_ns"].as_u64(),
                };
                let got = pipeline::export(&s)?;
                let cancelled_step = s.cancel_at_ns.is_some();
                let m = if cancelled_step {
                    let want_bytes = rec.result["bytes_written"].as_u64().unwrap_or(0);
                    got.cancelled
                        && got.header_sane == rec.result["header_sane"].as_bool().unwrap_or(false)
                        && got.bytes_written.abs_diff(want_bytes) <= CANCEL_BYTES_TOLERANCE
                } else {
                    got.sha256 == rec.result["sha256"].as_str().unwrap_or("")
                };
                println!(
                    "[replay #{:02}] {name}: {} (sha256={}, bytes={})",
                    rec.seq,
                    mark(m),
                    got.sha256,
                    got.bytes_written
                );
                ok &= m;
            }
            "verify-order" | "verify-repeat" => {
                let want = rec.result["matches_original"].as_bool().unwrap_or(false);
                println!("[replay #{:02}] {}: recorded matches_original={}", rec.seq, rec.step, want);
                ok &= want;
            }
            _ => {
                println!("[replay #{:02}] {}: skipped (non-executable step)", rec.seq, rec.step);
            }
        }
    }
    Ok(ok)
}

fn mark(m: bool) -> &'static str {
    if m {
        "MATCH"
    } else {
        "MISMATCH"
    }
}
