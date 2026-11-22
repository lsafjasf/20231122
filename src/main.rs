use anyhow::Result;
use clap::{Parser, Subcommand};
use safe_frame_audit::params::SafeFrameParams;
use safe_frame_audit::{detect, genmedia, png, record, replay, ui, waveform};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "safe-frame-audit", about = "画面安全框检测：可复核流程")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 生成确定性测试媒体（无损 raw/matroska）
    Generate {
        #[arg(long, default_value = "artifacts/sample.mkv")]
        out: PathBuf,
        #[arg(long, default_value_t = 100)]
        frames: u64,
    },
    /// 分析媒体并写检测记录（v1 旧格式 / v2 新格式）
    Analyze {
        #[arg(long)]
        media: PathBuf,
        #[arg(long)]
        record: PathBuf,
        #[arg(long, default_value = "v2")]
        format: String,
        #[arg(long, default_value = "artifacts/evidence")]
        evidence_dir: PathBuf,
    },
    /// 模拟崩溃：把记录截断 N 字节
    Truncate {
        #[arg(long)]
        record: PathBuf,
        #[arg(long, default_value_t = 23)]
        bytes: u64,
    },
    /// 恢复被截断的记录（保留 .truncated.bak 证据）
    Recover {
        #[arg(long)]
        record: PathBuf,
    },
    /// 恢复后重放：按记录参数重新分析并逐条比对，指出第一个错误状态
    Replay {
        #[arg(long)]
        record: PathBuf,
        #[arg(long)]
        media: Option<PathBuf>,
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// GTK4 复核界面
    Ui {
        #[arg(long)]
        record: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Generate { out, frames } => {
            if let Some(d) = out.parent() {
                std::fs::create_dir_all(d)?;
            }
            genmedia::generate(&out, frames)?;
            println!("生成媒体: {} ({} 帧)", out.display(), frames);
        }
        Cmd::Analyze {
            media,
            record,
            format,
            evidence_dir,
        } => {
            analyze(&media, &record, &format, &evidence_dir)?;
        }
        Cmd::Truncate { record, bytes } => {
            let raw = std::fs::read(&record)?;
            anyhow::ensure!(bytes < raw.len() as u64, "截断字节数超过文件长度");
            std::fs::write(&record, &raw[..raw.len() - bytes as usize])?;
            println!("已截断 {} 字节: {}", bytes, record.display());
        }
        Cmd::Recover { record } => {
            let mark = record::recover(&record)?;
            println!(
                "恢复完成: 丢弃 {} 字节垃圾尾, 续接 seq={}",
                mark.dropped_bytes, mark.resumed_seq
            );
            println!("证据保留: {}.truncated.bak", record.display());
        }
        Cmd::Replay {
            record,
            media,
            report,
        } => {
            let rep = replay::replay(&record, media.as_deref())?;
            let json = serde_json::to_string_pretty(&rep)?;
            if let Some(r) = &report {
                if let Some(d) = r.parent() {
                    std::fs::create_dir_all(d)?;
                }
                std::fs::write(r, &json)?;
            }
            println!("{json}");
            if let Some(fe) = &rep.first_error {
                println!(
                    "第一个错误状态: frame={} pts={}ms track={} metric={:.6}",
                    fe.frame, fe.pts_ms, fe.track, fe.metric
                );
            }
            if let Some(mm) = &rep.first_mismatch {
                println!(
                    "第一个不一致: seq={} frame={} kind={} 记录={:.6} 重放={:?}",
                    mm.seq, mm.frame, mm.kind, mm.recorded_metric, mm.replayed_metric
                );
            }
            std::process::exit(if rep.verdict == "OK" { 0 } else { 3 });
        }
        Cmd::Ui { record } => ui::run(record)?,
    }
    Ok(())
}

fn analyze(
    media: &PathBuf,
    record_path: &PathBuf,
    format: &str,
    evidence_dir: &PathBuf,
) -> Result<()> {
    let params = SafeFrameParams::default();
    std::fs::create_dir_all(evidence_dir)?;
    if let Some(d) = record_path.parent() {
        std::fs::create_dir_all(d)?;
    }

    let mut first_error_png: Option<String> = None;
    let mut entries: Vec<detect::FrameMetric> = Vec::new();
    let mut writer = if format == "v2" {
        Some(record::WriterV2::create(
            record_path,
            &media.to_string_lossy(),
            &params,
        )?)
    } else {
        None
    };
    let mut v1_entries = Vec::new();
    detect::analyze(media, &params, |fm, rgb, w, h| {
        if let Some(wr) = writer.as_mut() {
            wr.push(fm.pts_ms, fm.frame, "video0", fm.metric, fm.violation)
                .unwrap();
        } else {
            v1_entries.push(record::Entry {
                seq: fm.frame,
                pts_ms: fm.pts_ms,
                frame: fm.frame,
                track: "video0".into(),
                metric: fm.metric,
                violation: fm.violation,
                params_hash: String::new(),
            });
        }
        // 关键证据：第一个错误状态的叠加帧
        if fm.violation && first_error_png.is_none() {
            let over = detect::overlay_safe_rect(rgb, w, h, &params, true);
            let png = png::encode_rgb(w, h, &over);
            let p = evidence_dir.join(format!("first_error_frame_{}.png", fm.frame));
            std::fs::write(&p, png).unwrap();
            first_error_png = Some(p.display().to_string());
        }
        entries.push(fm.clone());
    })?;
    match writer {
        Some(w) => w.finish()?,
        None => record::write_v1(record_path, &v1_entries)?,
    }

    // 输出波形证据 PNG
    let wave: Vec<f64> = entries.iter().map(|e| e.metric).collect();
    let fe_idx = entries.iter().position(|e| e.violation);
    let wave_png = png::encode_rgb(
        waveform::WAVE_W,
        waveform::WAVE_H,
        &waveform::render(&wave, params.max_ratio, fe_idx),
    );
    let wave_path = evidence_dir.join("waveform.png");
    std::fs::write(&wave_path, wave_png)?;

    println!(
        "分析完成: {} 帧 -> {}",
        entries.len(),
        record_path.display()
    );
    if let Some(fe) = entries.iter().find(|e| e.violation) {
        println!(
            "第一个错误状态: frame={} pts={}ms metric={:.6}",
            fe.frame, fe.pts_ms, fe.metric
        );
        println!("证据帧: {}", first_error_png.clone().unwrap());
    } else {
        println!("未发现违规帧");
    }
    println!("输出波形: {}", wave_path.display());
    Ok(())
}
