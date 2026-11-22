use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::json;

use crate::discovery::{self, AudioProbe, ResolvedManifest};
use crate::failure::{Failure, Result};
use crate::gst_audio::{self, AudioAnalysis};
use crate::manifest::{self, CaseManifest};
use crate::report::{
    self, AutomatedReport, EvidenceFile, EvidenceIndex, ManualConfirmation, ManualVerdict, Verdict,
};
use crate::sha256;

#[derive(Debug, Clone, Serialize)]
pub struct VerificationReport {
    pub root: PathBuf,
    pub checked: usize,
    pub missing: usize,
    pub mismatched: usize,
    pub issues: Vec<String>,
    pub checked_at: String,
}

impl VerificationReport {
    pub fn ok(&self) -> bool {
        self.missing == 0 && self.mismatched == 0
    }
}

#[derive(Debug, Clone)]
pub struct AssessOptions {
    pub manifest: Option<PathBuf>,
    pub audio: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub copy_audio: bool,
    pub force: bool,
    pub max_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct AssessmentOutcome {
    pub success: bool,
    pub case_id: String,
    pub evidence_dir: PathBuf,
    pub manifest: Option<CaseManifest>,
    pub automated: Option<AutomatedReport>,
    pub manual: Option<ManualConfirmation>,
    pub analysis: Option<AudioAnalysis>,
    pub failure: Option<Failure>,
}

#[derive(Debug, Clone, Serialize)]
struct RunRecord {
    schema_version: u32,
    status: String,
    case_id: String,
    started_at: String,
    finished_at: Option<String>,
    evidence_dir: PathBuf,
    command: Vec<String>,
    manifest: Option<PathBuf>,
    audio: Option<PathBuf>,
    max_seconds: u64,
    failure: Option<Failure>,
}

#[derive(Debug, Clone, Serialize)]
struct InputInventory {
    schema_version: u32,
    audio: Option<AudioProbe>,
    manifest: Option<ManifestInventory>,
    environment: EnvironmentInfo,
}

#[derive(Debug, Clone, Serialize)]
struct ManifestInventory {
    path: PathBuf,
    sha256: String,
    size_bytes: u64,
    candidates: Vec<PathBuf>,
    detected_format: Option<String>,
    migrated: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct EnvironmentInfo {
    tool_version: String,
    gstreamer: String,
    gtk4: String,
    os: String,
}

pub fn assess(options: &AssessOptions) -> Result<AssessmentOutcome> {
    let started_at = chrono::Utc::now().to_rfc3339();
    let resolved =
        discovery::resolve_manifest(options.manifest.as_deref(), options.audio.as_deref());
    let case_hint = options
        .manifest
        .as_deref()
        .or(options.audio.as_deref())
        .and_then(Path::file_stem)
        .and_then(|value| value.to_str())
        .unwrap_or("assessment");
    let mut evidence_dir = prepare_directory(options.out.as_deref(), case_hint, options.force)?;
    let mut run_path = evidence_dir.join("run.json");
    let mut run = RunRecord {
        schema_version: 1,
        status: "running".to_string(),
        case_id: case_hint.to_string(),
        started_at: started_at.clone(),
        finished_at: None,
        evidence_dir: evidence_dir.clone(),
        command: std::env::args().collect(),
        manifest: options.manifest.clone(),
        audio: options.audio.clone(),
        max_seconds: options.max_seconds,
        failure: None,
    };
    write_json(&run_path, &run)?;

    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(failure) => {
            return finish_failure(options, &evidence_dir, &mut run, failure, None, None, None);
        }
    };
    let copied_manifest = match copy_manifest(&evidence_dir, resolved.as_ref()) {
        Ok(path) => path,
        Err(failure) => {
            return finish_failure(
                options,
                &evidence_dir,
                &mut run,
                failure,
                resolved,
                None,
                None,
            );
        }
    };

    let loaded = manifest::load_or_infer(resolved.clone(), options.audio.as_deref());
    let manifest = match loaded {
        Ok(manifest) => manifest,
        Err(failure) => {
            return finish_failure(
                options,
                &evidence_dir,
                &mut run,
                failure,
                resolved,
                copied_manifest,
                None,
            );
        }
    };
    run.case_id = manifest.case.case_id.clone();
    if options.out.is_none()
        && let Some(renamed) = rename_default_directory(&evidence_dir, &manifest.case.case_id)?
    {
        evidence_dir = renamed;
        run_path = evidence_dir.join("run.json");
        run.evidence_dir = evidence_dir.clone();
        write_json(&run_path, &run)?;
    }
    let mut probe = match discovery::probe_audio(&manifest.audio_path) {
        Ok(probe) => probe,
        Err(failure) => {
            return finish_failure(
                options,
                &evidence_dir,
                &mut run,
                failure,
                resolved,
                copied_manifest,
                Some(manifest),
            );
        }
    };
    let copied_audio = if options.copy_audio {
        match copy_audio(&evidence_dir, &manifest.audio_path) {
            Ok(path) => Some(path),
            Err(failure) => {
                return finish_failure(
                    options,
                    &evidence_dir,
                    &mut run,
                    failure,
                    resolved,
                    copied_manifest,
                    Some(manifest),
                );
            }
        }
    } else {
        None
    };

    write_manifest_snapshot(&evidence_dir, &manifest)?;
    let unknown_extension = probe.extension.clone().filter(|_| !probe.extension_known);
    let inventory_path = evidence_dir.join("input-inventory.json");
    let inventory = InputInventory {
        schema_version: 1,
        audio: Some(probe.clone()),
        manifest: manifest_inventory(resolved.as_ref(), Some(&manifest)),
        environment: environment_info(),
    };
    write_json(&inventory_path, &inventory)?;

    let output_wav = evidence_dir.join("output-waveform.wav");
    let analysis = match gst_audio::analyze_file(
        &manifest.audio_path,
        &output_wav,
        options.max_seconds.max(1),
    ) {
        Ok(analysis) => analysis,
        Err(failure) => {
            let _ = fs::remove_file(&output_wav);
            return finish_failure(
                options,
                &evidence_dir,
                &mut run,
                failure,
                resolved,
                copied_manifest,
                Some(manifest),
            );
        }
    };
    probe.gstreamer_caps = analysis.gstreamer_caps.clone();
    let inventory = InputInventory {
        schema_version: 1,
        audio: Some(probe.clone()),
        manifest: manifest_inventory(resolved.as_ref(), Some(&manifest)),
        environment: environment_info(),
    };
    write_json(&inventory_path, &inventory)?;
    let waveform_svg = evidence_dir.join("waveform.svg");
    crate::waveform::write_svg(&waveform_svg, &analysis.waveform, &manifest.events)?;
    let timeline_path = evidence_dir.join("timeline.csv");
    let automated = report::build_automated(
        &manifest,
        &analysis.waveform,
        unknown_extension.clone(),
        manifest.schema.migrated,
        EvidenceIndex {
            root: evidence_dir.clone(),
            ..EvidenceIndex::default()
        },
    );
    write_timeline(&timeline_path, &automated)?;
    let mut automated = automated;
    automated.evidence = collect_evidence_index(
        &evidence_dir,
        Some(&probe),
        copied_audio.as_deref(),
        copied_manifest.as_deref(),
        &inventory_path,
        &evidence_dir.join("normalized-manifest.json"),
        &waveform_svg,
        &timeline_path,
        &output_wav,
    )?;
    let mut manual = ManualConfirmation::initial(
        &manifest.case.case_id,
        &automated,
        unknown_extension.as_deref(),
    );
    if let Some(existing) = report::load_manual(&evidence_dir.join("manual-confirmation.json")) {
        manual.merge_recovered(&existing);
    }
    report::write_reports(&evidence_dir, &manifest, &automated, &manual)?;
    run.status = "completed".to_string();
    run.finished_at = Some(chrono::Utc::now().to_rfc3339());
    write_json(&run_path, &run)?;
    write_evidence_index(
        &evidence_dir,
        &automated,
        &manual,
        &run_path,
        Some(&probe),
        copied_audio.as_deref(),
        copied_manifest.as_deref(),
    )?;

    Ok(AssessmentOutcome {
        success: true,
        case_id: manifest.case.case_id.clone(),
        evidence_dir,
        manifest: Some(manifest),
        automated: Some(automated),
        manual: Some(manual),
        analysis: Some(analysis),
        failure: None,
    })
}

fn prepare_directory(requested: Option<&Path>, case_hint: &str, force: bool) -> Result<PathBuf> {
    let directory = requested.map(PathBuf::from).unwrap_or_else(|| {
        let safe_case = case_hint
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '-'
                }
            })
            .collect::<String>();
        PathBuf::from("evidence").join(format!(
            "{safe_case}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
        ))
    });
    if directory.exists() && !force {
        let non_empty = fs::read_dir(&directory)
            .map_err(|e| Failure::io("检查证据目录", &directory, e))?
            .next()
            .is_some();
        if non_empty {
            return Err(Failure::new(
                "evidence_exists",
                format!("证据目录非空：{}", directory.display()),
                "使用新的 `--out` 目录；确认要覆盖同名证据时增加 `--force`。",
            ));
        }
    }
    fs::create_dir_all(&directory).map_err(|e| Failure::io("创建证据目录", &directory, e))?;
    Ok(directory)
}

fn rename_default_directory(current: &Path, case_id: &str) -> Result<Option<PathBuf>> {
    let safe_case = case_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let Some(current_name) = current.file_name().and_then(|value| value.to_str()) else {
        return Ok(None);
    };
    if current_name.starts_with(&format!("{safe_case}-")) || safe_case.is_empty() {
        return Ok(None);
    }
    let parent = current.parent().unwrap_or_else(|| Path::new("."));
    let mut destination = parent.join(format!(
        "{safe_case}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
    ));
    let mut suffix = 1;
    while destination.exists() {
        destination = parent.join(format!(
            "{safe_case}-{}-{suffix}",
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
        ));
        suffix += 1;
    }
    fs::rename(current, &destination).map_err(|e| Failure::io("重命名证据目录", current, e))?;
    Ok(Some(destination))
}

#[allow(clippy::too_many_arguments)]
fn finish_failure(
    options: &AssessOptions,
    evidence_dir: &Path,
    run: &mut RunRecord,
    failure: Failure,
    resolved: Option<ResolvedManifest>,
    copied_manifest: Option<PathBuf>,
    manifest: Option<CaseManifest>,
) -> Result<AssessmentOutcome> {
    let audio_path = manifest
        .as_ref()
        .map(|manifest| manifest.audio_path.clone())
        .or_else(|| options.audio.clone());
    let probe = audio_path
        .as_deref()
        .filter(|path| path.is_file())
        .and_then(|path| discovery::probe_audio(path).ok());
    let copied_audio = if options.copy_audio {
        audio_path
            .as_deref()
            .filter(|path| path.is_file())
            .and_then(|path| copy_audio(evidence_dir, path).ok())
    } else {
        None
    };
    let unknown_extension = probe
        .as_ref()
        .and_then(|probe| probe.extension.clone().filter(|_| !probe.extension_known));
    let case_id = manifest
        .as_ref()
        .map(|manifest| manifest.case.case_id.clone())
        .or_else(|| {
            resolved.as_ref().and_then(|resolved| {
                resolved
                    .path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .map(str::to_string)
            })
        })
        .unwrap_or_else(|| run.case_id.clone());
    let inventory_path = evidence_dir.join("input-inventory.json");
    let inventory = InputInventory {
        schema_version: 1,
        audio: probe.clone(),
        manifest: manifest_inventory(resolved.as_ref(), manifest.as_ref()),
        environment: environment_info(),
    };
    write_json(&inventory_path, &inventory)?;
    if let Some(manifest) = manifest.as_ref() {
        write_manifest_snapshot(evidence_dir, manifest)?;
    }
    let mut automated = AutomatedReport::from_failure(
        &case_id,
        &failure,
        EvidenceIndex {
            root: evidence_dir.to_path_buf(),
            ..EvidenceIndex::default()
        },
    );
    automated.unknown_extension = unknown_extension.clone();
    let manual = ManualConfirmation::initial(&case_id, &automated, unknown_extension.as_deref());
    let display_manifest = manifest.unwrap_or_else(|| failure_manifest(&case_id, audio_path));
    let evidence = collect_evidence_index(
        evidence_dir,
        probe.as_ref(),
        copied_audio.as_deref(),
        copied_manifest.as_deref(),
        &inventory_path,
        &evidence_dir.join("normalized-manifest.json"),
        &evidence_dir.join("waveform.svg"),
        &evidence_dir.join("timeline.csv"),
        &evidence_dir.join("output-waveform.wav"),
    )?;
    automated.evidence = evidence;
    report::write_reports(evidence_dir, &display_manifest, &automated, &manual)?;
    run.status = "failed".to_string();
    run.case_id = case_id.clone();
    run.finished_at = Some(chrono::Utc::now().to_rfc3339());
    run.failure = Some(failure.clone());
    let run_path = evidence_dir.join("run.json");
    write_json(&run_path, &run)?;
    write_evidence_index(
        evidence_dir,
        &automated,
        &manual,
        &run_path,
        probe.as_ref(),
        copied_audio.as_deref(),
        copied_manifest.as_deref(),
    )?;
    Ok(AssessmentOutcome {
        success: false,
        case_id,
        evidence_dir: evidence_dir.to_path_buf(),
        manifest: Some(display_manifest),
        automated: Some(automated),
        manual: Some(manual),
        analysis: None,
        failure: Some(failure),
    })
}

fn failure_manifest(case_id: &str, audio_path: Option<PathBuf>) -> CaseManifest {
    CaseManifest {
        schema: manifest::SchemaInfo {
            detected_format: "unavailable".to_string(),
            input_version: "unknown".to_string(),
            normalized_version: 3,
            migrated: false,
            manifest_sha256: sha256::bytes_sha256(b""),
        },
        case: manifest::CaseInfo {
            case_id: case_id.to_string(),
            recovered_at: None,
            replay_origin: "recovery replay start (0:00)".to_string(),
            source_name: None,
            source_note: Some("清单加载失败；本报告仅记录可保留的失败证据。".to_string()),
        },
        audio_path: audio_path.unwrap_or_else(|| PathBuf::from("<unavailable-audio>")),
        tracks: Vec::new(),
        processing: Vec::new(),
        events: Vec::new(),
        warnings: vec!["评估未进入波形分析阶段。".to_string()],
    }
}

fn copy_manifest(
    evidence_dir: &Path,
    resolved: Option<&ResolvedManifest>,
) -> Result<Option<PathBuf>> {
    let Some(resolved) = resolved else {
        return Ok(None);
    };
    let directory = evidence_dir.join("inputs");
    fs::create_dir_all(&directory).map_err(|e| Failure::io("创建输入证据目录", &directory, e))?;
    let destination = directory.join("original-manifest.json");
    fs::copy(&resolved.path, &destination)
        .map_err(|e| Failure::io("复制原始清单", &resolved.path, e))?;
    let source_hash = sha256::file_sha256(&resolved.path)?;
    let copy_hash = sha256::file_sha256(&destination)?;
    if source_hash != copy_hash {
        return Err(Failure::new(
            "manifest_copy_mismatch",
            "原始清单复制后哈希不一致",
            "检查磁盘故障或并发修改；重新运行前先固定输入文件。",
        ));
    }
    Ok(Some(destination))
}

fn copy_audio(evidence_dir: &Path, audio: &Path) -> Result<PathBuf> {
    let directory = evidence_dir.join("inputs").join("audio");
    fs::create_dir_all(&directory).map_err(|e| Failure::io("创建音频证据目录", &directory, e))?;
    let name = audio
        .file_name()
        .ok_or_else(|| {
            Failure::new(
                "audio_name",
                format!("音频没有合法文件名：{}", audio.display()),
                "把文件重命名为有扩展名的普通文件后重试。",
            )
        })?
        .to_os_string();
    let destination = directory.join(name);
    if destination.exists() {
        fs::remove_file(&destination)
            .map_err(|e| Failure::io("替换旧音频证据", &destination, e))?;
    }
    fs::copy(audio, &destination).map_err(|e| Failure::io("复制原始音频证据", audio, e))?;
    let source_hash = sha256::file_sha256(audio)?;
    let copy_hash = sha256::file_sha256(&destination)?;
    if source_hash != copy_hash {
        return Err(Failure::new(
            "audio_copy_mismatch",
            "原始音频复制后哈希不一致",
            "检查磁盘故障或并发修改；重新运行前先固定输入文件。",
        ));
    }
    Ok(destination)
}

fn write_manifest_snapshot(evidence_dir: &Path, manifest: &CaseManifest) -> Result<()> {
    let path = evidence_dir.join("normalized-manifest.json");
    write_json(&path, manifest)
}

fn manifest_inventory(
    resolved: Option<&ResolvedManifest>,
    manifest: Option<&CaseManifest>,
) -> Option<ManifestInventory> {
    let resolved = resolved?;
    Some(ManifestInventory {
        path: resolved.path.clone(),
        sha256: sha256::file_sha256(&resolved.path).unwrap_or_else(|_| "unavailable".to_string()),
        size_bytes: resolved.bytes.len() as u64,
        candidates: resolved.candidates.clone(),
        detected_format: manifest.map(|manifest| manifest.schema.detected_format.clone()),
        migrated: manifest.map(|manifest| manifest.schema.migrated),
    })
}

fn environment_info() -> EnvironmentInfo {
    EnvironmentInfo {
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        gstreamer: gst::version_string().to_string(),
        gtk4: format!(
            "{}.{}.{}",
            gtk4::major_version(),
            gtk4::minor_version(),
            gtk4::micro_version()
        ),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_evidence_index(
    root: &Path,
    probe: Option<&AudioProbe>,
    copied_audio: Option<&Path>,
    copied_manifest: Option<&Path>,
    inventory: &Path,
    normalized_manifest: &Path,
    waveform_svg: &Path,
    timeline: &Path,
    output_wav: &Path,
) -> Result<EvidenceIndex> {
    let mut files = Vec::new();
    let mut original_audio = None;
    if let Some(copied) = copied_audio {
        let file = evidence_file(root, copied, "原始重放文件（原样保留，含未知扩展名）")?;
        original_audio = Some(file.clone());
        files.push(file);
    } else if let Some(probe) = probe {
        files.push(EvidenceFile {
            name: probe.file_name.clone(),
            path: probe.path.display().to_string(),
            sha256: Some(probe.sha256.clone()),
            description: "原始重放文件（未复制，按源路径记录）".to_string(),
        });
    }
    if let Some(copied) = copied_manifest {
        files.push(evidence_file(root, copied, "原始评估清单快照")?);
    }
    if inventory.is_file() {
        files.push(evidence_file(
            root,
            inventory,
            "输入文件、扩展名与工具环境清单",
        )?);
    }
    if normalized_manifest.is_file() {
        files.push(evidence_file(
            root,
            normalized_manifest,
            "归一化新旧格式后的可复核清单",
        )?);
    }
    if output_wav.is_file() {
        files.push(evidence_file(
            root,
            output_wav,
            "GStreamer 解码后的 48 kHz 单声道输出波形",
        )?);
    }
    if waveform_svg.is_file() {
        files.push(evidence_file(
            root,
            waveform_svg,
            "带事件时间标记的输出波形图",
        )?);
    }
    if timeline.is_file() {
        files.push(evidence_file(
            root,
            timeline,
            "事件、轨道/帧、参数、波形指标对照表",
        )?);
    }
    let unknown_extension = probe
        .and_then(|probe| probe.extension.clone())
        .filter(|_| probe.is_some_and(|probe| !probe.extension_known));
    Ok(EvidenceIndex {
        root: root.to_path_buf(),
        files,
        original_audio,
        unknown_extension,
    })
}

fn evidence_file(root: &Path, path: &Path, description: &str) -> Result<EvidenceFile> {
    let relative = path
        .strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf());
    Ok(EvidenceFile {
        name: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("artifact")
            .to_string(),
        path: relative.display().to_string(),
        sha256: path
            .is_file()
            .then(|| sha256::file_sha256(path))
            .transpose()?,
        description: description.to_string(),
    })
}

fn write_timeline(path: &Path, report: &AutomatedReport) -> Result<()> {
    let mut output = String::from(
        "sequence,event_id,start_ms,end_ms,track_id,track_index,frame,kind,processing_id,processing_parameters,output_waveform,peak_dbfs,rms_dbfs,silent_fraction,clipped_fraction,delta_db,auto_result,notes\n",
    );
    for (index, event) in report.events.iter().enumerate() {
        let (peak, rms, silent, clipped, delta) = event
            .metrics
            .as_ref()
            .map(|metrics| {
                (
                    format!("{:.3}", metrics.peak_dbfs),
                    format!("{:.3}", metrics.rms_dbfs),
                    format!("{:.6}", metrics.silent_fraction),
                    format!("{:.6}", metrics.clipped_fraction),
                    metrics
                        .delta_db
                        .map(|value| format!("{value:.3}"))
                        .unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        let row = [
            (index + 1).to_string(),
            event.event_id.clone(),
            format!("{:.3}", event.start_ms),
            format!("{:.3}", event.end_ms),
            event.track_id.clone().unwrap_or_default(),
            event
                .track_index
                .map(|value| value.to_string())
                .unwrap_or_default(),
            event
                .frame
                .map(|value| value.to_string())
                .unwrap_or_default(),
            event.kind.clone(),
            event.processing_id.clone().unwrap_or_default(),
            event.parameters.clone(),
            format!("output-waveform.wav#t={:.3}", event.start_ms / 1000.0),
            peak,
            rms,
            silent,
            clipped,
            delta,
            event.result.label().to_string(),
            event.notes.join("；"),
        ];
        output.push_str(
            &row.iter()
                .map(|value| csv_field(value))
                .collect::<Vec<_>>()
                .join(","),
        );
        output.push('\n');
    }
    fs::write(path, output).map_err(|e| Failure::io("写入时间线 CSV", path, e))
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn write_evidence_index(
    root: &Path,
    automated: &AutomatedReport,
    manual: &ManualConfirmation,
    run_path: &Path,
    probe: Option<&AudioProbe>,
    copied_audio: Option<&Path>,
    copied_manifest: Option<&Path>,
) -> Result<()> {
    let mut files = automated.evidence.files.clone();
    for (path, description) in [
        (run_path, "运行状态、命令、时间与失败原因"),
        (
            &*root.join("automated-conclusions.json"),
            "自动结论（独立证据）",
        ),
        (
            &*root.join("manual-confirmation.json"),
            "人工确认（独立证据）",
        ),
        (&*root.join("report.json"), "自动/人工合并的可机读报告"),
        (&*root.join("report.md"), "自动/人工分开呈现的可读报告"),
    ] {
        if path.is_file() {
            let file = evidence_file(root, path, description)?;
            if !files.iter().any(|existing| existing.path == file.path) {
                files.push(file);
            }
        }
    }
    let input_audio_sha256 = probe.map(|probe| probe.sha256.clone()).or_else(|| {
        automated
            .evidence
            .original_audio
            .as_ref()
            .and_then(|file| file.sha256.clone())
    });
    let index = json!({
        "schema_version": 1,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "case_id": automated.case_id,
        "unknown_extension": automated.unknown_extension,
        "input_audio_sha256": input_audio_sha256,
        "copied_audio": copied_audio.map(|path| path.display().to_string()),
        "copied_manifest": copied_manifest.map(|path| path.display().to_string()),
        "manual_status": manual.status,
        "files": files,
    });
    write_json(&root.join("evidence-index.json"), &index)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        Failure::new(
            "evidence_json",
            format!("序列化证据失败：{}", path.display()),
            "保留已有证据并重新运行；这通常表示内部数据异常。",
        )
        .with_detail(error.to_string())
    })?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| Failure::io("创建证据子目录", parent, e))?;
    }
    fs::write(path, bytes).map_err(|e| Failure::io("写入证据 JSON", path, e))
}

pub fn mark_manual_review(
    outcome: &mut AssessmentOutcome,
    reviewer: String,
    notes: String,
    verdicts: &[(String, ManualVerdict, String)],
) -> Result<()> {
    let Some(manifest) = &outcome.manifest else {
        return Err(Failure::new(
            "manual_without_case",
            "没有可关联的评估案件",
            "先成功加载清单并运行一次评估，再保存人工确认。",
        ));
    };
    let Some(automated) = &outcome.automated else {
        return Err(Failure::new(
            "manual_without_report",
            "没有自动结论可供人工复核",
            "先运行评估生成自动结论，再保存人工确认。",
        ));
    };
    let Some(manual) = &mut outcome.manual else {
        return Err(Failure::new(
            "manual_state",
            "人工确认状态不可用",
            "重新打开证据包后再次填写。",
        ));
    };
    manual.reviewer = reviewer;
    manual.notes = notes;
    manual.reviewed_at = Some(chrono::Utc::now().to_rfc3339());
    for (id, verdict, note) in verdicts {
        if let Some(item) = manual.items.iter_mut().find(|item| item.id == *id) {
            item.verdict = *verdict;
            item.note = note.clone();
        }
    }
    manual.recompute_status();
    report::update_manual_report(&outcome.evidence_dir, manifest, automated, manual)?;
    write_json(
        &outcome.evidence_dir.join("manual-confirmation.json"),
        manual,
    )?;
    write_evidence_index(
        &outcome.evidence_dir,
        automated,
        manual,
        &outcome.evidence_dir.join("run.json"),
        None,
        None,
        None,
    )?;
    Ok(())
}

pub fn overall_auto_verdict(outcome: &AssessmentOutcome) -> Verdict {
    outcome
        .automated
        .as_ref()
        .map(|report| report.verdict)
        .unwrap_or(Verdict::Inconclusive)
}

pub fn verify_evidence(root: &Path) -> Result<VerificationReport> {
    let index_path = root.join("evidence-index.json");
    if !index_path.is_file() {
        return Err(Failure::missing_file(
            "证据索引",
            &index_path,
            "确认 `--evidence` 指向评估生成的证据包根目录，而不是其父目录。",
        ));
    }
    let bytes = fs::read(&index_path).map_err(|e| Failure::io("读取证据索引", &index_path, e))?;
    let index: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| Failure::json("证据索引", &index_path, error))?;
    let files = index
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            Failure::new(
                "evidence_index_shape",
                "证据索引缺少 `files` 数组",
                "保留该文件供排查；重新运行评估生成新的 evidence-index.json。",
            )
        })?;
    let mut checked = 0;
    let mut missing = 0;
    let mut mismatched = 0;
    let mut issues = Vec::new();
    for file in files {
        let Some(expected) = file.get("sha256").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(path_text) = file.get("path").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let path = PathBuf::from(path_text);
        let path = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        checked += 1;
        if !path.is_file() {
            missing += 1;
            issues.push(format!("缺失：{}", path.display()));
            continue;
        }
        let actual = sha256::file_sha256(&path)?;
        if actual != expected {
            mismatched += 1;
            issues.push(format!(
                "哈希不一致：{}（期望 {}，实际 {}）",
                path.display(),
                expected,
                actual
            ));
        }
    }
    let report_md = root.join("report.md");
    if report_md.is_file() {
        let markdown = fs::read_to_string(&report_md)
            .map_err(|e| Failure::io("读取可读报告", &report_md, e))?;
        if !markdown.contains("## 自动结论") || !markdown.contains("## 人工确认") {
            issues.push("report.md 未同时包含自动结论与人工确认两部分。".to_string());
        }
    } else {
        issues.push("缺少 report.md。".to_string());
    }
    let report = VerificationReport {
        root: root.to_path_buf(),
        checked,
        missing,
        mismatched,
        issues,
        checked_at: chrono::Utc::now().to_rfc3339(),
    };
    let verification_path = root.join("verification.json");
    write_json(
        &verification_path,
        &serde_json::json!({
            "schema_version": 1,
            "ok": report.ok(),
            "report": report,
        }),
    )?;
    Ok(report)
}
