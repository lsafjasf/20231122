//! 引擎端到端测试：真实 GStreamer 解码、来源登记与临时状态清理。

mod common;

use std::fs;
use std::path::PathBuf;

use fadecheck::engine_public::run_batch;
use fadecheck::model_public::{SourceKind, Verdict};

fn ensure_gst() {
    // GStreamer 初始化是幂等的；每个测试进程至少调用一次。
    let _ = fadecheck::decoder_public::init();
}

struct FixtureDir {
    root: PathBuf,
}

impl FixtureDir {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "fadecheck-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn write_wav(&self, name: &str, samples: &[f32], rate: u32) -> PathBuf {
        let path = self.root.join(name);
        let data = render_wav(samples, rate);
        fs::write(&path, data).unwrap();
        path
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn render_wav(samples: &[f32], rate: u32) -> Vec<u8> {
    let data_bytes = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_bytes);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36u32 + data_bytes as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        out.extend_from_slice(&((clamped * 32767.0) as i16).to_le_bytes());
    }
    out
}

#[test]
fn batch_covers_unknown_extension_boundary_and_evidence_chain() {
    ensure_gst();
    let fixture = FixtureDir::new();
    let boundary = fixture.write_wav(
        "boundary.wav",
        &common::fixture_samples(common::FadeShape::BoundaryBand),
        48_000,
    );
    let faded = fixture.write_wav(
        "faded.wav",
        &common::fixture_samples(common::FadeShape::FullFade),
        48_000,
    );

    // 未知扩展：内容其实是音频，但来源检查策略不允许猜测媒体类型。
    let unknown = fixture.root.join("clip.xyz");
    fs::copy(&faded, &unknown).unwrap();
    let missing = fixture.root.join("nope.wav");

    let inputs = [
        unknown.display().to_string(),
        boundary.display().to_string(),
        faded.display().to_string(),
        missing.display().to_string(),
    ];
    let (report, temp) = run_batch(&inputs);

    assert_eq!(report.total_objects, 4);
    assert_eq!(report.conclusions.len(), 4);

    let unknown_conclusion = &report.conclusions[0];
    assert_eq!(unknown_conclusion.verdict, Verdict::NotAnalyzable);
    assert!(unknown_conclusion
        .warnings
        .iter()
        .any(|w| w.contains("未知扩展")));
    assert!(unknown_conclusion.evidence[0].note.contains("SHA-256"));

    let boundary_conclusion = &report.conclusions[1];
    assert_eq!(boundary_conclusion.verdict, Verdict::Inconclusive);
    assert_eq!(boundary_conclusion.snapshots.len(), 2);
    assert!(boundary_conclusion
        .evidence
        .iter()
        .all(|e| e.threshold.contains("边界带")));
    // 两个快照必须指向同一个原始输入指纹。
    let hash = &boundary_conclusion.snapshots[0].input_sha256;
    assert!(hash.len() == 64);
    assert_eq!(hash, &boundary_conclusion.snapshots[1].input_sha256);
    // v1 保留原始 48 kHz，v2 重采样到 16 kHz：版本差异必须体现在快照里。
    assert_eq!(boundary_conclusion.snapshots[0].sample_rate, Some(48_000));
    assert_eq!(boundary_conclusion.snapshots[1].sample_rate, Some(16_000));
    assert!(
        boundary_conclusion.snapshots[0].frame_count.unwrap()
            > boundary_conclusion.snapshots[1].frame_count.unwrap()
    );

    let faded_conclusion = &report.conclusions[2];
    assert_eq!(faded_conclusion.verdict, Verdict::FadeInAndOut);
    assert_eq!(faded_conclusion.snapshots.len(), 2);

    let missing_conclusion = &report.conclusions[3];
    assert_eq!(missing_conclusion.verdict, Verdict::NotAnalyzable);

    // 结束时清理临时状态。
    temp.cleanup();
    assert!(!temp.path().exists());

    // SourceKind 仅用于登记阶段，确认枚举可被外部序列化使用。
    let _ = SourceKind::UnknownExtension;
}
