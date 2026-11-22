use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const PROJECT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SegmentStatus {
    Idle,
    Running,
    Ready,
    Warning,
    Interrupted,
    Failed,
}

impl SegmentStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "未运行",
            Self::Running => "运行中",
            Self::Ready => "已修正",
            Self::Warning => "需复核",
            Self::Interrupted => "已中断",
            Self::Failed => "失败",
        }
    }

    pub fn css(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Ready => "ready",
            Self::Warning => "warning",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtremeParams {
    pub preset: String,
    pub threshold_dbfs: f32,
    pub ceiling_dbfs: f32,
    pub makeup_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub boundary_window_ms: u32,
}

impl Default for ExtremeParams {
    fn default() -> Self {
        Self {
            preset: "极限".into(),
            threshold_dbfs: -6.0,
            ceiling_dbfs: -1.0,
            makeup_db: 9.0,
            attack_ms: 2.0,
            release_ms: 80.0,
            boundary_window_ms: 60,
        }
    }
}

impl ExtremeParams {
    pub fn extreme() -> Self {
        Self::default()
    }

    pub fn restrained() -> Self {
        Self {
            preset: "稳健".into(),
            threshold_dbfs: -3.0,
            ceiling_dbfs: -1.5,
            makeup_db: 3.0,
            attack_ms: 8.0,
            release_ms: 140.0,
            boundary_window_ms: 80,
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "阈值 {:.1} / 上限 {:.1} / 增益 {:+.1} dB",
            self.threshold_dbfs, self.ceiling_dbfs, self.makeup_db
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeakSample {
    pub at_ms: i64,
    pub peak_dbfs: f32,
    pub rms_dbfs: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BoundarySamples {
    pub head_before: Option<PeakSample>,
    pub head_after: Option<PeakSample>,
    pub tail_before: Option<PeakSample>,
    pub tail_after: Option<PeakSample>,
}

impl BoundarySamples {
    pub fn ordered(&self) -> [(&'static str, Option<&PeakSample>); 4] {
        [
            ("入口前", self.head_before.as_ref()),
            ("入口后", self.head_after.as_ref()),
            ("出口前", self.tail_before.as_ref()),
            ("出口后", self.tail_after.as_ref()),
        ]
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SegmentAnalysis {
    pub peak_dbfs: f32,
    pub rms_dbfs: f32,
    pub warning_count: usize,
    pub sample_count: usize,
    pub boundaries: BoundarySamples,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvidenceSampleChange {
    pub boundary: String,
    pub before: Option<PeakSample>,
    pub after: Option<PeakSample>,
    pub delta_db: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CorrectionRun {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub params: ExtremeParams,
    pub output_path: String,
    pub duration_ms: u64,
    pub original: SegmentAnalysis,
    pub corrected: SegmentAnalysis,
    pub clipped_samples: u64,
    pub headroom_recovered_db: f32,
    pub boundary_changes: Vec<EvidenceSampleChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    pub id: String,
    pub index: usize,
    pub start_ms: u64,
    pub end_ms: u64,
    pub label: String,
    pub missing: bool,
    pub status: SegmentStatus,
    pub last_error: Option<String>,
    pub runs: Vec<CorrectionRun>,
}

impl Segment {
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }

    pub fn latest(&self) -> Option<&CorrectionRun> {
        self.runs.last()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvidenceEvent {
    pub at: DateTime<Utc>,
    pub kind: String,
    pub segment_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub version: u32,
    pub id: String,
    pub name: String,
    pub media_path: String,
    pub media_duration_ms: u64,
    pub sample_rate: u32,
    pub channels: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub selected_segment: Option<String>,
    pub params: ExtremeParams,
    pub segments: Vec<Segment>,
    pub evidence: Vec<EvidenceEvent>,
}

impl Project {
    pub fn new_demo(media_path: PathBuf) -> Self {
        let now = Utc::now();
        Self {
            version: PROJECT_VERSION,
            id: "PG-C07".into(),
            name: "现场录音 C-07 · 局部修正".into(),
            media_path: media_path.to_string_lossy().into_owned(),
            media_duration_ms: 24_000,
            sample_rate: 48_000,
            channels: 1,
            created_at: now,
            updated_at: now,
            selected_segment: Some("seg-03".into()),
            params: ExtremeParams::default(),
            segments: vec![
                demo_segment(1, 0, 4_000, "开场环境", false),
                demo_segment(2, 4_000, 8_000, "近场峰值", false),
                demo_segment(3, 8_000, 12_000, "缺失片段", true),
                demo_segment(4, 12_000, 16_000, "瞬态爆音", false),
                demo_segment(5, 16_000, 20_000, "收尾人声", false),
                demo_segment(6, 20_000, 24_000, "尾部余量", false),
            ],
            evidence: vec![EvidenceEvent {
                at: now,
                kind: "session".into(),
                segment_id: None,
                message: "项目已创建；已锁定缺失片段与边界采样窗口。".into(),
            }],
        }
    }
}

fn demo_segment(index: usize, start_ms: u64, end_ms: u64, label: &str, missing: bool) -> Segment {
    Segment {
        id: format!("seg-{index:02}"),
        index,
        start_ms,
        end_ms,
        label: label.into(),
        missing,
        status: SegmentStatus::Idle,
        last_error: None,
        runs: Vec::new(),
    }
}

impl Project {
    pub fn from_media(
        path: &Path,
        duration_ms: u64,
        sample_rate: u32,
        channels: u32,
        segment_count: usize,
    ) -> Self {
        let now = Utc::now();
        let count = segment_count.max(1);
        let duration_ms = duration_ms.max(500);
        let mut segments = Vec::with_capacity(count);
        for index in 0..count {
            let start_ms = duration_ms * index as u64 / count as u64;
            let end_ms = duration_ms * (index + 1) as u64 / count as u64;
            segments.push(Segment {
                id: format!("seg-{:02}", index + 1),
                index: index + 1,
                start_ms,
                end_ms,
                label: format!("媒体片段 {:02}", index + 1),
                missing: false,
                status: SegmentStatus::Idle,
                last_error: None,
                runs: Vec::new(),
            });
        }
        let name = path
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("未命名媒体");
        Self {
            version: PROJECT_VERSION,
            id: format!("PG-{:06}", now.timestamp() % 1_000_000),
            name: format!("{name} · 峰值修正"),
            media_path: path.to_string_lossy().into_owned(),
            media_duration_ms: duration_ms,
            sample_rate,
            channels,
            created_at: now,
            updated_at: now,
            selected_segment: segments.first().map(|segment| segment.id.clone()),
            params: ExtremeParams::default(),
            segments,
            evidence: vec![EvidenceEvent {
                at: now,
                kind: "import".into(),
                segment_id: None,
                message: format!("导入媒体，按 {} 个片段建立独立任务；原始文件只读。", count),
            }],
        }
    }

    pub fn selected(&self) -> Option<&Segment> {
        let id = self.selected_segment.as_deref()?;
        self.segments.iter().find(|segment| segment.id == id)
    }

    pub fn select(&mut self, segment_id: &str) {
        if self.segments.iter().any(|segment| segment.id == segment_id) {
            self.selected_segment = Some(segment_id.to_owned());
            self.touch();
        }
    }

    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }

    pub fn add_evidence(
        &mut self,
        kind: impl Into<String>,
        segment_id: Option<String>,
        message: impl Into<String>,
    ) {
        self.evidence.push(EvidenceEvent {
            at: Utc::now(),
            kind: kind.into(),
            segment_id,
            message: message.into(),
        });
        if self.evidence.len() > 300 {
            let drop_count = self.evidence.len() - 300;
            self.evidence.drain(0..drop_count);
        }
        self.touch();
    }

    pub fn recover_interrupted(&mut self) -> Vec<String> {
        let mut recovered = Vec::new();
        for segment in &mut self.segments {
            if segment.status == SegmentStatus::Running {
                segment.status = SegmentStatus::Interrupted;
                recovered.push(segment.id.clone());
            }
        }
        if !recovered.is_empty() {
            let message = if recovered.len() == 1 {
                "检测到上次运行在片段处理中退出；状态已恢复为“已中断”，原始与本段已有证据均可继续使用。"
                    .to_owned()
            } else {
                format!(
                    "检测到 {} 个片段在上次退出时仍在运行；已恢复为“已中断”，可逐段重跑。",
                    recovered.len()
                )
            };
            self.add_evidence("recovery", None, message);
        }
        recovered
    }

    pub fn apply_run(&mut self, segment_id: &str, run: CorrectionRun) {
        if let Some(segment) = self
            .segments
            .iter_mut()
            .find(|segment| segment.id == segment_id)
        {
            let passed = run.corrected.warning_count == 0 && !segment.missing;
            let output = Path::new(&run.output_path)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("导出文件");
            let summary = format!(
                "片段 {:02} 完成：峰值 {:.1} → {:.1} dBFS，截断 {} 个样本，输出 {}。",
                segment.index,
                run.original.peak_dbfs,
                run.corrected.peak_dbfs,
                run.clipped_samples,
                output
            );
            segment.status = if passed {
                SegmentStatus::Ready
            } else {
                SegmentStatus::Warning
            };
            segment.last_error = None;
            segment.runs.push(run);
            if segment.runs.len() > 12 {
                let drop_count = segment.runs.len() - 12;
                segment.runs.drain(0..drop_count);
            }
            self.add_evidence("run", Some(segment_id.to_owned()), summary);
        }
        self.touch();
    }
}

#[derive(Debug, Clone)]
pub struct ProjectStore {
    root: PathBuf,
}

impl Default for ProjectStore {
    fn default() -> Self {
        let root = std::env::var_os("PEAKGUARD_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("artifacts/peakguard")
            });
        Self { root }
    }
}

impl ProjectStore {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn session_path(&self) -> PathBuf {
        self.root.join("session.json")
    }

    pub fn segments_dir(&self) -> PathBuf {
        self.root.join("segments")
    }

    pub fn exports_dir(&self) -> PathBuf {
        self.root.join("exports")
    }

    pub fn demo_media_path(&self) -> PathBuf {
        self.root.join("demo/site-c07.wav")
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("创建数据目录失败: {}", self.root.display()))?;
        fs::create_dir_all(self.segments_dir())?;
        fs::create_dir_all(self.exports_dir())?;
        if let Some(parent) = self.demo_media_path().parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    pub fn load(&self) -> Result<Option<Project>> {
        let path = self.session_path();
        if !path.exists() {
            return Ok(None);
        }
        let data = fs::read_to_string(&path)
            .with_context(|| format!("读取会话失败: {}", path.display()))?;
        let mut project: Project =
            serde_json::from_str(&data).with_context(|| "会话文件格式损坏，无法恢复")?;
        if project.version > PROJECT_VERSION {
            anyhow::bail!("会话版本高于当前程序支持版本");
        }
        project.recover_interrupted();
        Ok(Some(project))
    }

    pub fn save(&self, project: &Project) -> Result<()> {
        self.ensure_dirs()?;
        let path = self.session_path();
        let temp = path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(project)?;
        fs::write(&temp, data).with_context(|| format!("写入临时会话失败: {}", temp.display()))?;
        fs::rename(&temp, &path).with_context(|| format!("提交会话失败: {}", path.display()))?;
        Ok(())
    }

    pub fn segment_output_path(&self, segment: &Segment) -> PathBuf {
        self.segments_dir()
            .join(format!("segment-{:02}-corrected.wav", segment.index))
    }
}

pub fn format_time_ms(ms: u64) -> String {
    let total_seconds = ms / 1000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    let millis = ms % 1000;
    if millis >= 10 {
        format!("{minutes:02}:{seconds:02}.{:02}", millis / 10)
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_segment_is_recovered_as_interrupted() {
        let root =
            std::env::temp_dir().join(format!("peakguard-store-test-{}", std::process::id()));
        let store = ProjectStore { root: root.clone() };
        let mut project = Project::new_demo(root.join("demo.wav"));
        project.segments[1].status = SegmentStatus::Running;
        store.save(&project).unwrap();

        let recovered = store.load().unwrap().unwrap();
        assert_eq!(recovered.segments[1].status, SegmentStatus::Interrupted);
        assert!(
            recovered
                .evidence
                .iter()
                .any(|event| event.kind == "recovery")
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
