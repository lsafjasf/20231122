use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail};
use clockbridge::compare::{compare_attempts, store_comparison};
use clockbridge::export::{export_review_bundle, load_review_bundle, verify_review_bundle};
use clockbridge::model::{AttemptRecord, AttemptStatus, MissingSegment, Workspace, format_time_ms};
use clockbridge::runner::{self, RunRequest, apply_run_result};
use clockbridge::store::{WORKSPACE_FILE, WorkspaceStore};

fn main() {
    if let Err(error) = execute() {
        eprintln!("错误: {error:#}");
        std::process::exit(1);
    }
}

fn execute() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        print_help();
        return Ok(());
    };
    match command {
        "demo" => command_demo(&args[1..]),
        "show" => command_show(&args[1..]),
        "set-gap" => command_set_gap(&args[1..]),
        "run" => command_run(&args[1..]),
        "compare" => command_compare(&args[1..]),
        "export" => command_export(&args[1..]),
        "verify" => command_verify(&args[1..]),
        "inspect-bundle" => command_inspect_bundle(&args[1..]),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => bail!("未知命令 {other}；运行 retryctl help 查看用法"),
    }
}

fn command_demo(args: &[String]) -> Result<()> {
    let root = PathBuf::from(option(args, "--root").unwrap_or("retry-workspace"));
    let workspace_file = root.join(WORKSPACE_FILE);
    if workspace_file.exists() && !has_flag(args, "--force") {
        bail!(
            "工作区已存在: {}（如需覆盖请加 --force）",
            workspace_file.display()
        );
    }
    if workspace_file.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    let device_a = option(args, "--a").unwrap_or("examples/media/device-a.mp4");
    let device_b = option(args, "--b").unwrap_or("examples/media/device-b.mp4");
    let workspace = Workspace::demo(device_a, device_b);
    let store = WorkspaceStore::create(&root, workspace)?;
    println!("已创建工作区: {}", store.workspace_path().display());
    println!("设备 A: {device_a}");
    println!("设备 B: {device_b}");
    println!(
        "B 时钟偏移: +{} ms",
        store.workspace().devices[1].clock_offset.milliseconds
    );
    Ok(())
}

fn command_show(args: &[String]) -> Result<()> {
    let store = open_store(args)?;
    let workspace = store.workspace();
    println!("{}", workspace.title);
    println!(
        "工作区 {} · revision {} · {} 条证据",
        workspace.workspace_id,
        workspace.revision,
        store.events().len()
    );
    for device in &workspace.devices {
        println!(
            "  {} [{}] offset={}ms path={}",
            device.name, device.id, device.clock_offset.milliseconds, device.media_path
        );
    }
    for segment in &workspace.segments {
        println!(
            "\n#{} {}  timeline={} duration={}  attempts={} comparisons={}",
            segment.ordinal,
            segment.label,
            format_time_ms(segment.timeline_start_ms),
            format_time_ms(segment.duration_ms),
            segment.attempts.len(),
            segment.comparisons.len()
        );
        for source in &segment.sources {
            println!(
                "  source {}: {} -> {} (offset {}ms)",
                source.device_id,
                format_time_ms(source.resolved_start_ms),
                format_time_ms(source.resolved_end_ms),
                source.clock_offset_ms
            );
        }
        for attempt in &segment.attempts {
            println!(
                "  attempt #{:03} {} {} wall={}ms size={:?} sha={}",
                attempt.attempt_no,
                attempt.device_id,
                attempt.status.label(),
                attempt.wall_time_ms,
                attempt.output_size_bytes,
                attempt
                    .output_sha256
                    .as_deref()
                    .map(short_hash)
                    .unwrap_or("-")
            );
        }
    }
    Ok(())
}

fn command_set_gap(args: &[String]) -> Result<()> {
    let mut store = open_store(args)?;
    let start_ms = parse_i64(option(args, "--start-ms").unwrap_or("2800"), "--start-ms")?;
    let duration_ms = parse_i64(
        option(args, "--duration-ms").unwrap_or("1600"),
        "--duration-ms",
    )?;
    let offset_b_ms = parse_i64(
        option(args, "--offset-b-ms").unwrap_or("1250"),
        "--offset-b-ms",
    )?;
    let label = option(args, "--label").unwrap_or("GAP-001").to_string();
    let reason = option(args, "--reason")
        .unwrap_or("双机时钟不同步产生的缺失片段")
        .to_string();
    {
        let workspace = store.workspace_mut();
        workspace.devices[1].clock_offset.milliseconds = offset_b_ms;
        if let Some(path) = option(args, "--b") {
            workspace.devices[1].media_path = path.to_string();
        }
        if let Some(path) = option(args, "--a") {
            workspace.devices[0].media_path = path.to_string();
        }
        workspace.set_missing_segment(MissingSegment {
            label: label.clone(),
            timeline_start_ms: start_ms,
            duration_ms,
            reason,
        });
    }
    store.save()?;
    store.record_event(
        "missing_segment_updated",
        None,
        None,
        "更新缺失片段与时钟偏移",
        vec![
            format!("片段: {label}"),
            format!("主时间线起点: {start_ms} ms"),
            format!("时长: {duration_ms} ms"),
            format!("设备 B 偏移: {offset_b_ms:+} ms"),
        ],
    )?;
    println!("已保存缺失片段 {label} 与双机时钟参数");
    Ok(())
}

fn command_run(args: &[String]) -> Result<()> {
    runner::initialize().map_err(anyhow::Error::msg)?;
    let mut store = open_store(args)?;
    let segment_selector = option(args, "--segment");
    let device_selector = option(args, "--device");
    let (segment_id, segment_label, ordinal, binding, output_path, extension) = {
        let workspace = store.workspace();
        let segment = match segment_selector {
            Some(selector) => workspace
                .segments
                .iter()
                .find(|segment| segment.id == selector || segment.label == selector)
                .with_context(|| format!("找不到片段 {selector}"))?,
            None => workspace.segments.first().context("工作区没有片段")?,
        };
        let binding = match device_selector {
            Some(selector) => segment
                .sources
                .iter()
                .find(|source| source.device_id == selector || source.device_name == selector)
                .with_context(|| format!("片段中没有设备 {selector}"))?,
            None => segment.sources.first().context("片段没有源绑定")?,
        };
        let extension = runner::available_output_extension();
        let attempt_no = segment.next_attempt_no();
        let output_path = store
            .root()
            .join("runs")
            .join(format!("segment-{:03}", segment.ordinal))
            .join(format!(
                "attempt-{:03}-{}.{}",
                attempt_no, binding.device_id, extension
            ));
        (
            segment.id.clone(),
            segment.label.clone(),
            segment.ordinal,
            binding.clone(),
            output_path,
            extension,
        )
    };
    let _ = extension;
    let evidence_file = "evidence/attempts/pending.json".to_string();
    let (attempt_id, request) = {
        let workspace = store.workspace_mut();
        let segment = workspace
            .segments
            .iter_mut()
            .find(|segment| segment.id == segment_id)
            .context("片段在运行前消失")?;
        let attempt = AttemptRecord::new_pending(
            segment,
            &binding,
            output_path.to_string_lossy().as_ref(),
            &evidence_file,
        );
        let attempt_id = attempt.id.clone();
        let request = RunRequest {
            source_path: PathBuf::from(&binding.media_path),
            output_path: output_path.clone(),
            start_ms: binding.clamped_start_ms(),
            duration_ms: binding.clamped_duration_ms(),
            dot_path: None,
        };
        segment.attempts.push(attempt);
        workspace.touch();
        (attempt_id, request)
    };
    {
        let workspace = store.workspace_mut();
        let segment = workspace
            .segments
            .iter_mut()
            .find(|segment| segment.id == segment_id)
            .context("片段消失")?;
        let attempt = segment
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == attempt_id)
            .context("尝试消失")?;
        attempt.evidence_file = format!("evidence/attempts/{attempt_id}.json");
    }
    store.save()?;
    store.record_event(
        "attempt_started",
        Some(segment_id.clone()),
        Some(attempt_id.clone()),
        "开始单独重跑媒体片段",
        vec![
            format!("片段: {segment_label}"),
            format!("设备: {}", binding.device_name),
            format!("源: {}", binding.media_path),
            format!(
                "范围: {} → {}",
                format_time_ms(binding.resolved_start_ms),
                format_time_ms(binding.resolved_end_ms)
            ),
        ],
    )?;
    println!(
        "运行 {} / {} · 第 {} 段 · {} ms",
        segment_label,
        binding.device_id,
        ordinal,
        binding.clamped_duration_ms()
    );
    let result = runner::run_segment(&request, &AtomicBool::new(false), |position| {
        if position % 2_000 < 100 {
            eprintln!("  进度: {} ms", position);
        }
    });
    let result_evidence_path = store
        .root()
        .join("evidence")
        .join("attempts")
        .join(format!("{attempt_id}.result.json"));
    if let Err(error) = runner::write_run_evidence(
        &result_evidence_path,
        &attempt_id,
        &segment_id,
        &request.source_path,
        request.start_ms,
        request.duration_ms,
        &result,
    ) {
        eprintln!("警告: {error}");
    }
    let (status, error, evidence_file) = {
        let workspace = store.workspace_mut();
        let segment = workspace
            .segments
            .iter_mut()
            .find(|segment| segment.id == segment_id)
            .context("片段消失")?;
        let attempt = segment
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == attempt_id)
            .context("尝试消失")?;
        apply_run_result(attempt, result);
        let summary = (
            attempt.status,
            attempt.error.clone(),
            attempt.evidence_file.clone(),
        );
        if attempt.status == AttemptStatus::Succeeded {
            segment.selected_attempt_id = Some(attempt_id.clone());
        }
        summary
    };
    store.workspace_mut().touch();
    store.save()?;
    let attempt_snapshot = store
        .workspace()
        .segment(&segment_id)
        .and_then(|segment| {
            segment
                .attempts
                .iter()
                .find(|attempt| attempt.id == attempt_id)
        })
        .cloned();
    if let Some(attempt) = attempt_snapshot {
        store.write_attempt_evidence(&attempt_id, &attempt)?;
    }
    store.record_event(
        if status == AttemptStatus::Succeeded {
            "attempt_succeeded"
        } else {
            "attempt_failed"
        },
        Some(segment_id),
        Some(attempt_id),
        if status == AttemptStatus::Succeeded {
            "片段重跑成功并写入产物指纹"
        } else {
            "片段重跑未成功，保留失败证据"
        },
        vec![
            format!("状态: {}", status.label()),
            format!("证据文件: {evidence_file}"),
            format!("错误: {}", error.unwrap_or_else(|| "无".to_string())),
        ],
    )?;
    println!("结果: {} · 证据 {}", status.label(), evidence_file);
    Ok(())
}

fn command_compare(args: &[String]) -> Result<()> {
    let mut store = open_store(args)?;
    let segment_id = option(args, "--segment").map(str::to_string);
    let left_id = option(args, "--left").map(str::to_string);
    let right_id = option(args, "--right").map(str::to_string);
    let comparison = {
        let workspace = store.workspace_mut();
        let segment = match segment_id.as_deref() {
            Some(selector) => workspace
                .segments
                .iter_mut()
                .find(|segment| segment.id == selector || segment.label == selector)
                .with_context(|| format!("找不到片段 {selector}"))?,
            None => workspace.segments.first_mut().context("工作区没有片段")?,
        };
        let left = left_id
            .as_deref()
            .and_then(|id| segment.attempts.iter().find(|attempt| attempt.id == id))
            .or_else(|| segment.attempts.last())
            .context("没有可比较的尝试")?
            .clone();
        let right = right_id
            .as_deref()
            .and_then(|id| segment.attempts.iter().find(|attempt| attempt.id == id))
            .or_else(|| {
                segment
                    .attempts
                    .iter()
                    .rev()
                    .find(|attempt| attempt.device_id != left.device_id)
            })
            .or_else(|| segment.attempts.first())
            .context("没有可比较的尝试")?
            .clone();
        let comparison = compare_attempts(&left, &right);
        store_comparison(segment, comparison.clone())?;
        workspace.touch();
        comparison
    };
    store.save()?;
    store.record_event(
        "comparison_recorded",
        None,
        None,
        "记录尝试比较结论",
        vec![
            format!("比较 ID: {}", comparison.id),
            format!("结论: {}", comparison.verdict),
        ],
    )?;
    println!(
        "比较 {} ↔ {}",
        comparison.left_attempt_id, comparison.right_attempt_id
    );
    println!("{}", comparison.verdict);
    Ok(())
}

fn command_export(args: &[String]) -> Result<()> {
    let store = open_store(args)?;
    let out = option(args, "--out").context("缺少 --out <新导出目录>")?;
    let include_outputs = !has_flag(args, "--no-outputs");
    let report = export_review_bundle(store.workspace(), store.events(), out, include_outputs)?;
    println!("复核包: {}", report.bundle_dir.display());
    println!("清单: {}", report.manifest_path.display());
    println!(
        "文件 {} 个，含产物副本 {} 个",
        report.file_count, report.copied_outputs
    );
    println!("校验: sha256sum -c {}", report.checksums_path.display());
    Ok(())
}

fn command_verify(args: &[String]) -> Result<()> {
    let bundle = args
        .first()
        .context("缺少复核包目录；用法: retryctl verify <bundle>")?;
    let manifest = load_review_bundle(bundle)?;
    let report = verify_review_bundle(bundle)?;
    println!(
        "schema={} tool={} {}",
        manifest.schema_version, manifest.tool_name, manifest.tool_version
    );
    println!(
        "复核: {} 个文件、{} 次尝试、{} 条比较、{} 条事件",
        report.checked_files, report.attempts, report.comparisons, report.evidence_events
    );
    if report.is_ok() {
        println!("结果: 通过");
        Ok(())
    } else {
        for error in &report.errors {
            eprintln!("- {error}");
        }
        bail!("复核包校验失败（{} 项）", report.errors.len())
    }
}

fn command_inspect_bundle(args: &[String]) -> Result<()> {
    let bundle = args
        .first()
        .context("缺少复核包目录；用法: retryctl inspect-bundle <bundle>")?;
    let manifest = load_review_bundle(bundle)?;
    println!("{}", serde_json::to_string_pretty(&manifest)?);
    Ok(())
}

fn open_store(args: &[String]) -> Result<WorkspaceStore> {
    let path = option(args, "--workspace")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("retry-workspace").join(WORKSPACE_FILE));
    let workspace_path = if path.is_dir() {
        path.join(WORKSPACE_FILE)
    } else {
        path
    };
    WorkspaceStore::open(&workspace_path)
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}

fn parse_i64(value: &str, name: &str) -> Result<i64> {
    value
        .parse::<i64>()
        .with_context(|| format!("{name} 需要整数毫秒，收到 {value}"))
}

fn short_hash(value: &str) -> &str {
    value.get(..12).unwrap_or(value)
}

fn print_help() {
    println!(
        "ClockBridge 局部修正工具\n\n\
         用法:\n\
           retryctl demo [--root DIR] [--a FILE] [--b FILE] [--force]\n\
           retryctl show --workspace <workspace.json>\n\
           retryctl set-gap --workspace <...> --start-ms 2800 --duration-ms 1600 --offset-b-ms 1250\n\
           retryctl run --workspace <...> [--segment LABEL|ID] [--device ID]\n\
           retryctl compare --workspace <...> [--segment ...] [--left ID] [--right ID]\n\
           retryctl export --workspace <...> --out <新目录> [--no-outputs]\n\
           retryctl verify <bundle>\n\
           retryctl inspect-bundle <bundle>"
    );
}
