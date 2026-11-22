use crate::journal::JournalWriter;
use crate::model::RawEvent;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::path::Path;
use std::time::{Duration, Instant};

pub struct RecordOptions {
    pub channels: u32,
    pub frames: u64,
    pub tick_ms: u64,
    /// 在这些 seq 上故意不写日志，模拟丢帧
    pub drop_seqs: Vec<u64>,
    /// 在该 seq 处写半行后中止，模拟记录被截断
    pub truncate_at: Option<u64>,
    /// 在该 seq 前人为 sleep，制造同步漂移
    pub stall_at: Option<u64>,
    pub stall_ms: u64,
}

fn wall_ms(start: &Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

/// 用 GStreamer 多路 videotestsrc 采集帧，逐帧写 JSONL 日志。
pub fn record(path: &Path, opt: &RecordOptions) -> Result<(), String> {
    gst::init().map_err(|e| e.to_string())?;
    let mut w = JournalWriter::create(path).map_err(|e| e.to_string())?;
    let session_id = format!(
        "sess-{}",
        path.file_stem().unwrap().to_string_lossy()
    );
    let t0 = Instant::now();
    w.write_event(&RawEvent::SessionStart {
        session_id: session_id.clone(),
        channels: opt.channels,
        tick_ms: opt.tick_ms,
        started_ms: 0,
    })
    .map_err(|e| e.to_string())?;

    let mut sinks = Vec::new();
    let mut pipes = Vec::new();
    for ch in 0..opt.channels {
        let launch = format!(
            "videotestsrc is-live=true pattern={} ! video/x-raw,framerate={}/1 ! videoconvert ! appsink name=s max-buffers=2 drop=true",
            (ch % 24) as i32,
            1000 / opt.tick_ms.max(1)
        );
        let pipe = gst::parse::launch(&launch)
            .map_err(|e| e.to_string())?
            .downcast::<gst::Pipeline>()
            .map_err(|_| "not a pipeline".to_string())?;
        let sink = pipe
            .by_name("s")
            .ok_or("no appsink")?
            .downcast::<gst_app::AppSink>()
            .map_err(|_| "not appsink")?;
        pipe.set_state(gst::State::Playing).map_err(|e| e.to_string())?;
        sinks.push(sink);
        pipes.push(pipe);
    }

    for seq in 0..opt.frames {
        if let Some(stall) = opt.stall_at {
            if stall == seq {
                std::thread::sleep(Duration::from_millis(opt.stall_ms));
            }
        }
        for (ch, sink) in sinks.iter().enumerate() {
            let sample = sink
                .pull_sample()
                .map_err(|e| format!("pull sample ch{}: {}", ch, e))?;
            let pts = sample
                .buffer()
                .and_then(|b| b.pts())
                .map(|p| p.mseconds())
                .unwrap_or(0);
            let ev = RawEvent::Frame {
                channel: ch as u32,
                seq,
                pts_ms: pts,
                wall_ms: wall_ms(&t0),
            };
            if opt.drop_seqs.contains(&seq) && ch as u32 == 1 {
                continue; // 模拟通道 1 丢帧
            }
            if opt.truncate_at == Some(seq) && ch as u32 == 0 {
                w.write_partial(&ev).map_err(|e| e.to_string())?;
                for p in &pipes {
                    let _ = p.set_state(gst::State::Null);
                }
                // 模拟崩溃：不写 SessionEnd，直接返回
                return Ok(());
            }
            w.write_event(&ev).map_err(|e| e.to_string())?;
        }
        // 每 10 帧写一次 checkpoint，供恢复重放定位
        if seq % 10 == 9 {
            for ch in 0..opt.channels {
                w.write_event(&RawEvent::Checkpoint {
                    channel: ch,
                    last_seq: seq,
                    wall_ms: wall_ms(&t0),
                })
                .map_err(|e| e.to_string())?;
            }
        }
    }
    w.write_event(&RawEvent::SessionEnd {
        session_id,
        wall_ms: wall_ms(&t0),
    })
    .map_err(|e| e.to_string())?;
    for p in &pipes {
        let _ = p.set_state(gst::State::Null);
    }
    Ok(())
}

/// 恢复后的重放：从 checkpoint 续采剩余帧，写入新的重放日志。
pub fn replay(
    original: &Path,
    replay_path: &Path,
    opt: &RecordOptions,
) -> Result<(), String> {
    let lines = crate::journal::read_journal(original).map_err(|e| e.to_string())?;
    let cps = crate::journal::last_checkpoints(&lines);
    let resume_seq = cps.values().map(|(s, _)| *s).min().unwrap_or(0) + 1;
    let mut w = JournalWriter::create(replay_path).map_err(|e| e.to_string())?;
    w.write_event(&RawEvent::Recovered {
        session_id: format!(
            "replay-{}",
            replay_path.file_stem().unwrap().to_string_lossy()
        ),
        resumed_from: original.file_name().unwrap().to_string_lossy().to_string(),
    })
    .map_err(|e| e.to_string())?;
    let mut sub = RecordOptions {
        frames: opt.frames,
        truncate_at: None,
        stall_at: None,
        drop_seqs: vec![],
        channels: opt.channels,
        tick_ms: opt.tick_ms,
        stall_ms: 0,
    };
    // 重放只补 checkpoint 之后的帧；seq 从 resume_seq 开始编号
    sub.frames = opt.frames;
    replay_range(replay_path, &mut w, &sub, resume_seq)
}

fn replay_range(
    _path: &Path,
    w: &mut JournalWriter,
    opt: &RecordOptions,
    from_seq: u64,
) -> Result<(), String> {
    gst::init().map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let mut sinks = Vec::new();
    let mut pipes = Vec::new();
    for ch in 0..opt.channels {
        let launch = format!(
            "videotestsrc is-live=true pattern={} ! video/x-raw,framerate={}/1 ! videoconvert ! appsink name=s max-buffers=2 drop=true",
            (ch % 24) as i32,
            1000 / opt.tick_ms.max(1)
        );
        let pipe = gst::parse::launch(&launch)
            .map_err(|e| e.to_string())?
            .downcast::<gst::Pipeline>()
            .map_err(|_| "not a pipeline".to_string())?;
        let sink = pipe
            .by_name("s")
            .ok_or("no appsink")?
            .downcast::<gst_app::AppSink>()
            .map_err(|_| "not appsink")?;
        pipe.set_state(gst::State::Playing).map_err(|e| e.to_string())?;
        sinks.push(sink);
        pipes.push(pipe);
    }
    for seq in from_seq..opt.frames {
        for (ch, sink) in sinks.iter().enumerate() {
            let sample = sink
                .pull_sample()
                .map_err(|e| format!("pull sample ch{}: {}", ch, e))?;
            let pts = sample
                .buffer()
                .and_then(|b| b.pts())
                .map(|p| p.mseconds())
                .unwrap_or(0);
            w.write_event(&RawEvent::Frame {
                channel: ch as u32,
                seq,
                pts_ms: pts,
                wall_ms: wall_ms(&t0),
            })
            .map_err(|e| e.to_string())?;
        }
    }
    w.write_event(&RawEvent::SessionEnd {
        session_id: "replay".into(),
        wall_ms: wall_ms(&t0),
    })
    .map_err(|e| e.to_string())?;
    for p in &pipes {
        let _ = p.set_state(gst::State::Null);
    }
    Ok(())
}
