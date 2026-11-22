//! 演示数据：生成含噪 WAV，真实运行分析并记录两次中途取消、一次完成、一次失败。

use crate::analysis;
use crate::model::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const DEMO_WAV: &str = "artifacts/demo_noise.wav";

/// 生成 4 秒 44.1kHz 16bit 单声道 WAV：白噪声 + 50Hz 工频哼声 + 1kHz 音调。
pub fn generate_wav(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let rate = 44_100u32;
    let secs = 4u32;
    let n = (rate * secs) as usize;
    let mut rng: u32 = 0x1234_5678;
    let mut pcm = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f64 / rate as f64;
        rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = ((rng >> 8) as f64 / (1 << 24) as f64 - 0.5) * 0.10;
        let hum = (2.0 * std::f64::consts::PI * 50.0 * t).sin() * 0.35;
        let tone = (2.0 * std::f64::consts::PI * 1000.0 * t).sin() * 0.20;
        // 边界信号：5kHz 弱音调，幅度贴近判定阈值，用于产生“无法判断”频段
        let edge = (2.0 * std::f64::consts::PI * 5000.0 * t).sin() * 0.030;
        let sample = (noise + hum + tone + edge).clamp(-1.0, 1.0);
        pcm.extend_from_slice(&((sample * 32767.0) as i16).to_le_bytes());
    }
    let data_len = pcm.len() as u32;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(&pcm);
    std::fs::write(path, wav)
}

fn make_record(seq: usize, input: &Path, cancel_after_ms: Option<u64>) -> RunRecord {
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(ms) = cancel_after_ms {
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            flag.store(true, Ordering::SeqCst);
        });
    }
    let digest = analysis::sha256_file(input).unwrap_or_default();
    let size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let outcome = analysis::run_analysis(input, cancel, None);
    let cancelled = outcome.status == RunStatus::Cancelled;
    RunRecord {
        id: format!("R{seq:03}"),
        input_path: input.display().to_string(),
        input_sha256: digest,
        input_size_bytes: size,
        created_ts: now_ts(),
        status: outcome.status,
        progress: outcome.progress,
        cancel_reason: if cancelled {
            Some("用户中途取消".to_string())
        } else {
            None
        },
        error: outcome.error,
        next_actions: outcome.next_actions,
        steps: outcome.steps,
        conclusions: outcome.conclusions,
        reviews: Vec::new(),
    }
}

/// 生成演示日志：两次中途取消 + 一次完成 + 一次失败（真实 GStreamer 处理）。
pub fn build_demo_journal() -> Result<Journal, String> {
    gst_init_once();
    let wav = PathBuf::from(DEMO_WAV);
    generate_wav(&wav).map_err(|e| format!("生成演示音频失败: {e}"))?;

    let mut journal = Journal::default();
    // 两次中途取消（约 35% 与约 70% 处）
    journal.runs.push(make_record(1, &wav, Some(1_400)));
    journal.runs.push(make_record(2, &wav, Some(2_800)));
    // 一次完整分析
    journal.runs.push(make_record(3, &wav, None));
    // 一次失败（输入不存在），用于展示可执行后续操作
    journal.runs
        .push(make_record(4, Path::new("artifacts/missing_input.wav"), None));

    journal
        .save(&journal_path())
        .map_err(|e| format!("写入日志失败: {e}"))?;
    Ok(journal)
}

fn gst_init_once() {
    gstreamer::init().ok();
}
