use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use gtk::prelude::*;
use gtk4 as gtk;

use super::{AppState, JobOutcome, view};
use clockbridge::compare::{compare_attempts, store_comparison};
use clockbridge::export::export_review_bundle;
use clockbridge::model::{
    AttemptRecord, AttemptStatus, ComparisonRecord, MissingSegment, format_time_ms,
};
use clockbridge::runner::{self, RunRequest, apply_run_result};
use clockbridge::store::WorkspaceStore;

pub(crate) fn connect_window_events(app: &Rc<RefCell<AppState>>) {
    let list = { app.borrow().segment_list.clone() };
    let list_app = app.clone();
    list.connect_row_activated(move |_, row| {
        let id = row.widget_name().to_string();
        if id.is_empty() {
            return;
        }
        {
            let mut state = list_app.borrow_mut();
            if state.selected_segment_id.as_deref() == Some(id.as_str()) {
                return;
            }
            state.selected_segment_id = Some(id);
        }
        view::refresh(&list_app);
    });

    let window = { app.borrow().window.clone() };
    let close_app = app.clone();
    window.connect_close_request(move |_| {
        let mut state = close_app.borrow_mut();
        state.cancel.store(true, Ordering::Relaxed);
        let _ = state.store.save();
        let revision = state.store.workspace().revision;
        let pending_jobs = state.running_jobs.len();
        let _ = state.store.record_event(
            "application_exit",
            None,
            None,
            "窗口关闭：工作区已落盘，运行中任务等待下次载入时标记为中断",
            vec![
                format!("revision: {revision}"),
                format!("未完成任务: {pending_jobs}"),
            ],
        );
        gtk::glib::Propagation::Proceed
    });
}

pub(crate) fn start_job_poll(app: &Rc<RefCell<AppState>>) {
    let poll_app = app.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(120), move || {
        let (changed, messages) = {
            let mut state = poll_app.borrow_mut();
            poll_jobs(&mut state)
        };
        if changed {
            view::refresh(&poll_app);
        }
        for (heading, detail) in messages {
            show_error(&poll_app, &heading, &detail);
        }
        gtk::glib::ControlFlow::Continue
    });
}

fn poll_jobs(state: &mut AppState) -> (bool, Vec<(String, String)>) {
    let mut changed = false;
    let mut messages = Vec::new();
    while let Ok(job) = state.rx.try_recv() {
        state.running_jobs.remove(&job.attempt_id);
        let mut finished_snapshot = None;
        let mut event_kind = "attempt_failed";
        let mut event_summary = "片段重跑未成功，失败证据已保留";
        let mut details = vec![format!("状态: {}", job.result.status.label())];
        {
            let workspace = state.store.workspace_mut();
            if let Some(segment) = workspace
                .segments
                .iter_mut()
                .find(|segment| segment.id == job.segment_id)
                && let Some(attempt) = segment
                    .attempts
                    .iter_mut()
                    .find(|attempt| attempt.id == job.attempt_id)
            {
                apply_run_result(attempt, job.result);
                if attempt.status == AttemptStatus::Succeeded
                    && segment.selected_attempt_id.is_none()
                {
                    segment.selected_attempt_id = Some(attempt.id.clone());
                }
                details.push(format!("设备: {}", attempt.device_name));
                details.push(format!(
                    "范围: {} + {} ms",
                    format_time_ms(attempt.resolved_start_ms),
                    attempt.duration_ms
                ));
                details.push(format!(
                    "输入 SHA-256: {}",
                    attempt.source_sha256.as_deref().unwrap_or("未取得")
                ));
                details.push(format!(
                    "输出 SHA-256: {}",
                    attempt.output_sha256.as_deref().unwrap_or("未取得")
                ));
                details.push(format!("管线: {}", attempt.pipeline));
                details.push(format!(
                    "错误: {}",
                    attempt.error.as_deref().unwrap_or("无")
                ));
                if attempt.status == AttemptStatus::Succeeded {
                    event_kind = "attempt_succeeded";
                    event_summary = "片段重跑成功并写入产物指纹";
                }
                finished_snapshot = Some(attempt.clone());
            }
            workspace.touch();
        }
        if let Err(error) = state.store.save() {
            messages.push(("保存尝试结果失败".to_string(), format!("{error:#}")));
        }
        if let Some(attempt) = &finished_snapshot
            && let Err(error) = state.store.write_attempt_evidence(&job.attempt_id, attempt)
        {
            messages.push(("写入尝试证据失败".to_string(), format!("{error:#}")));
        }
        if let Err(error) = state.store.record_event(
            event_kind,
            Some(job.segment_id),
            Some(job.attempt_id),
            event_summary,
            details,
        ) {
            messages.push(("追加证据日志失败".to_string(), format!("{error:#}")));
        }
        changed = true;
    }
    (changed, messages)
}

pub(crate) fn choose_media(app: &Rc<RefCell<AppState>>, device_id: &'static str) {
    let window = { app.borrow().window.clone() };
    let dialog = gtk::FileDialog::builder()
        .title("选择媒体文件")
        .modal(true)
        .build();
    let choose_app = app.clone();
    dialog.open(
        Some(&window),
        None::<&gtk::gio::Cancellable>,
        move |result| {
            let Ok(file) = result else {
                return;
            };
            let Some(path) = file.path() else {
                return;
            };
            let result = update_device_media(&choose_app, device_id, &path);
            if let Err(error) = result {
                show_error(&choose_app, "更新媒体路径失败", &format!("{error:#}"));
            }
        },
    );
}

fn update_device_media(
    app: &Rc<RefCell<AppState>>,
    device_id: &str,
    path: &std::path::Path,
) -> Result<()> {
    {
        let mut state = app.borrow_mut();
        let workspace = state.store.workspace_mut();
        let device = workspace
            .devices
            .iter_mut()
            .find(|device| device.id == device_id)
            .context("设备不存在")?;
        device.media_path = path.to_string_lossy().to_string();
        workspace.refresh_sources();
        state.store.save()?;
        state.store.record_event(
            "media_path_updated",
            None,
            None,
            "更新设备媒体文件",
            vec![
                format!("设备: {device_id}"),
                format!("路径: {}", path.display()),
            ],
        )?;
    }
    view::refresh(app);
    Ok(())
}

pub(crate) fn open_workspace_dialog(app: &Rc<RefCell<AppState>>) {
    let window = { app.borrow().window.clone() };
    let dialog = gtk::FileDialog::builder()
        .title("打开 workspace.json")
        .modal(true)
        .build();
    let open_app = app.clone();
    dialog.open(
        Some(&window),
        None::<&gtk::gio::Cancellable>,
        move |result| {
            let Ok(file) = result else {
                return;
            };
            let Some(path) = file.path() else {
                return;
            };
            match WorkspaceStore::open(&path) {
                Ok(store) => {
                    {
                        let mut state = open_app.borrow_mut();
                        state.store = store;
                        state.selected_segment_id = None;
                    }
                    view::refresh(&open_app);
                }
                Err(error) => {
                    show_error(&open_app, "打开工作区失败", &format!("{error:#}"));
                }
            }
        },
    );
}

pub(crate) fn apply_missing_range(app: &Rc<RefCell<AppState>>) {
    let result = (|| -> Result<()> {
        let mut state = app.borrow_mut();
        let start_ms = state.start_ms.value().round() as i64;
        let duration_ms = state.duration_ms.value().round() as i64;
        if duration_ms <= 0 {
            bail!("缺口时长必须大于 0");
        }
        let offset_b = state.offset_b.value().round() as i64;
        let reason = state.reason.text().to_string();
        let label = state
            .store
            .workspace()
            .missing_segment
            .as_ref()
            .map(|missing| missing.label.clone())
            .unwrap_or_else(|| "GAP-001".to_string());
        {
            let workspace = state.store.workspace_mut();
            if let Some(device_b) = workspace.devices.get_mut(1) {
                device_b.clock_offset.milliseconds = offset_b;
            }
            workspace.set_missing_segment(MissingSegment {
                label: label.clone(),
                timeline_start_ms: start_ms,
                duration_ms,
                reason,
            });
        }
        state.store.save()?;
        state.store.record_event(
            "missing_segment_updated",
            None,
            None,
            "更新双机时钟与缺失片段范围",
            vec![
                format!("片段: {label}"),
                format!("主时间线起点: {start_ms} ms"),
                format!("时长: {duration_ms} ms"),
                format!("设备 B 偏移: {offset_b:+} ms"),
            ],
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => view::refresh(app),
        Err(error) => show_error(app, "无法应用缺失范围", &format!("{error:#}")),
    }
}

pub(crate) fn rerun_source(app: &Rc<RefCell<AppState>>, segment_id: &str, device_id: &str) {
    let result = {
        let mut state = app.borrow_mut();
        enqueue_attempt(&mut state, segment_id, device_id)
    };
    match result {
        Ok(()) => view::refresh(app),
        Err(error) => show_error(app, "无法启动片段重跑", &format!("{error:#}")),
    }
}

pub(crate) fn rerun_segment(app: &Rc<RefCell<AppState>>, segment_id: &str) {
    let devices: Vec<String> = {
        let state = app.borrow();
        state
            .store
            .workspace()
            .segment(segment_id)
            .map(|segment| {
                segment
                    .sources
                    .iter()
                    .map(|source| source.device_id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let mut errors = Vec::new();
    for device_id in devices {
        let result = {
            let mut state = app.borrow_mut();
            enqueue_attempt(&mut state, segment_id, &device_id)
        };
        if let Err(error) = result {
            errors.push(format!("{device_id}: {error:#}"));
        }
    }
    view::refresh(app);
    if !errors.is_empty() {
        show_error(app, "部分片段未能启动", &errors.join("\n"));
    }
}

fn enqueue_attempt(state: &mut AppState, segment_id: &str, device_id: &str) -> Result<()> {
    let extension = runner::available_output_extension();
    let root = state.store.root().to_path_buf();
    let (attempt_id, request, segment_label) = {
        let workspace = state.store.workspace_mut();
        let segment = workspace
            .segments
            .iter_mut()
            .find(|segment| segment.id == segment_id)
            .context("片段不存在")?;
        let binding = segment
            .sources
            .iter()
            .find(|source| source.device_id == device_id)
            .context("片段未绑定该设备")?
            .clone();
        let attempt_no = segment.next_attempt_no();
        let output_path = root
            .join("runs")
            .join(format!("segment-{:03}", segment.ordinal))
            .join(format!(
                "attempt-{:03}-{}.{}",
                attempt_no, binding.device_id, extension
            ));
        let mut attempt = AttemptRecord::new_pending(
            segment,
            &binding,
            output_path.to_string_lossy().as_ref(),
            "",
        );
        let attempt_id = attempt.id.clone();
        attempt.evidence_file = format!("evidence/attempts/{attempt_id}.json");
        let request = RunRequest {
            source_path: PathBuf::from(&binding.media_path),
            output_path: output_path.clone(),
            start_ms: binding.clamped_start_ms(),
            duration_ms: binding.clamped_duration_ms(),
            dot_path: Some(
                root.join("evidence")
                    .join("attempts")
                    .join(format!("{attempt_id}.dot")),
            ),
        };
        let segment_label = segment.label.clone();
        segment.attempts.push(attempt);
        workspace.touch();
        (attempt_id, request, segment_label)
    };
    state.store.save()?;
    if let Err(error) = state.store.record_event(
        "attempt_started",
        Some(segment_id.to_string()),
        Some(attempt_id.clone()),
        "开始单独重跑媒体片段",
        vec![
            format!("片段: {segment_label}"),
            format!("设备: {device_id}"),
            format!("源: {}", request.source_path.display()),
            format!(
                "请求范围: {} ms + {} ms",
                request.start_ms, request.duration_ms
            ),
        ],
    ) {
        {
            let workspace = state.store.workspace_mut();
            if let Some(attempt) = workspace
                .segments
                .iter_mut()
                .find(|segment| segment.id == segment_id)
                .and_then(|segment| {
                    segment
                        .attempts
                        .iter_mut()
                        .find(|attempt| attempt.id == attempt_id)
                })
            {
                attempt.finish(
                    AttemptStatus::Failed,
                    Some("证据日志写入失败，任务未启动".to_string()),
                );
            }
            workspace.touch();
        }
        let _ = state.store.save();
        return Err(error);
    }

    state.running_jobs.insert(attempt_id.clone());
    let tx = state.tx.clone();
    let cancel = state.cancel.clone();
    let result_evidence_path = root
        .join("evidence")
        .join("attempts")
        .join(format!("{attempt_id}.result.json"));
    let thread_segment_id = segment_id.to_string();
    let worker_attempt_id = attempt_id.clone();
    let spawned = thread::Builder::new()
        .name(format!("segment-{}", short_id(&attempt_id)))
        .spawn(move || {
            let result = runner::run_segment(&request, &cancel, |_| {});
            let _ = runner::write_run_evidence(
                &result_evidence_path,
                &worker_attempt_id,
                &thread_segment_id,
                &request.source_path,
                request.start_ms,
                request.duration_ms,
                &result,
            );
            let _ = tx.send(JobOutcome {
                segment_id: thread_segment_id,
                attempt_id: worker_attempt_id,
                result,
            });
        });
    if let Err(error) = spawned {
        state.running_jobs.remove(&attempt_id);
        {
            let workspace = state.store.workspace_mut();
            if let Some(attempt) = workspace
                .segments
                .iter_mut()
                .find(|segment| segment.id == segment_id)
                .and_then(|segment| {
                    segment
                        .attempts
                        .iter_mut()
                        .find(|attempt| attempt.id == attempt_id)
                })
            {
                attempt.finish(
                    AttemptStatus::Failed,
                    Some("无法创建后台重跑线程".to_string()),
                );
            }
            workspace.touch();
        }
        let _ = state.store.save();
        return Err(error).context("创建后台重跑线程失败");
    }
    Ok(())
}

pub(crate) fn select_baseline(app: &Rc<RefCell<AppState>>, segment_id: &str, attempt_id: &str) {
    let result = (|| -> Result<()> {
        let mut state = app.borrow_mut();
        {
            let workspace = state.store.workspace_mut();
            let segment = workspace
                .segments
                .iter_mut()
                .find(|segment| segment.id == segment_id)
                .context("片段不存在")?;
            let attempt = segment
                .attempts
                .iter()
                .find(|attempt| attempt.id == attempt_id)
                .context("尝试不存在")?;
            if attempt.status != AttemptStatus::Succeeded {
                bail!("仅成功的尝试可以设为基准");
            }
            segment.selected_attempt_id = Some(attempt_id.to_string());
            workspace.touch();
        }
        state.store.save()?;
        state.store.record_event(
            "baseline_selected",
            Some(segment_id.to_string()),
            Some(attempt_id.to_string()),
            "选择片段修复基准尝试",
            Vec::new(),
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => view::refresh(app),
        Err(error) => show_error(app, "无法设置基准", &format!("{error:#}")),
    }
}

pub(crate) fn compare_latest(app: &Rc<RefCell<AppState>>, segment_id: &str) {
    let ids = {
        let state = app.borrow();
        state
            .store
            .workspace()
            .segment(segment_id)
            .map(|segment| {
                segment
                    .attempts
                    .iter()
                    .rev()
                    .take(2)
                    .map(|attempt| attempt.id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    if ids.len() < 2 {
        show_error(app, "无法比较", "至少需要两次尝试记录。");
        return;
    }
    compare_pair(app, segment_id, &ids[1], &ids[0]);
}

pub(crate) fn compare_pair(
    app: &Rc<RefCell<AppState>>,
    segment_id: &str,
    left_id: &str,
    right_id: &str,
) {
    let result = (|| -> Result<ComparisonRecord> {
        let mut state = app.borrow_mut();
        let comparison = {
            let workspace = state.store.workspace_mut();
            let segment = workspace
                .segments
                .iter_mut()
                .find(|segment| segment.id == segment_id)
                .context("片段不存在")?;
            let left = segment
                .attempts
                .iter()
                .find(|attempt| attempt.id == left_id)
                .context("左侧尝试不存在")?
                .clone();
            let right = segment
                .attempts
                .iter()
                .find(|attempt| attempt.id == right_id)
                .context("右侧尝试不存在")?
                .clone();
            let comparison = compare_attempts(&left, &right);
            store_comparison(segment, comparison.clone())?;
            workspace.touch();
            comparison
        };
        state.store.save()?;
        state.store.record_event(
            "comparison_recorded",
            Some(segment_id.to_string()),
            None,
            "记录两条尝试的比较结论",
            vec![
                format!("比较 ID: {}", comparison.id),
                format!("左: {left_id}"),
                format!("右: {right_id}"),
                format!("结论: {}", comparison.verdict),
                format!("大小差: {:?}", comparison.size_delta_bytes),
                format!("校验和一致: {}", comparison.checksum_equal),
            ],
        )?;
        Ok(comparison)
    })();
    match result {
        Ok(_) => view::refresh(app),
        Err(error) => show_error(app, "无法记录比较", &format!("{error:#}")),
    }
}

pub(crate) fn export_bundle_dialog(app: &Rc<RefCell<AppState>>) {
    let window = { app.borrow().window.clone() };
    let dialog = gtk::FileDialog::builder()
        .title("选择复核包输出位置")
        .modal(true)
        .build();
    let export_app = app.clone();
    dialog.select_folder(
        Some(&window),
        None::<&gtk::gio::Cancellable>,
        move |result| {
            let Ok(folder) = result else {
                return;
            };
            let Some(parent) = folder.path() else {
                return;
            };
            let destination = parent.join(format!(
                "clockbridge-review-{}",
                Utc::now().format("%Y%m%d-%H%M%S")
            ));
            run_export(&export_app, destination, None);
        },
    );
}

pub(crate) fn export_segment_dialog(app: &Rc<RefCell<AppState>>, segment_id: &str) {
    let window = { app.borrow().window.clone() };
    let segment_label = {
        let state = app.borrow();
        state
            .store
            .workspace()
            .segment(segment_id)
            .map(|segment| segment.label.clone())
            .unwrap_or_else(|| "segment".to_string())
    };
    let dialog = gtk::FileDialog::builder()
        .title("选择单段复核包输出位置")
        .modal(true)
        .build();
    let export_app = app.clone();
    let segment_id = segment_id.to_string();
    dialog.select_folder(
        Some(&window),
        None::<&gtk::gio::Cancellable>,
        move |result| {
            let Ok(folder) = result else {
                return;
            };
            let Some(parent) = folder.path() else {
                return;
            };
            let destination = parent.join(format!(
                "clockbridge-{}-{}",
                sanitize_file_component(&segment_label),
                Utc::now().format("%Y%m%d-%H%M%S")
            ));
            run_export(&export_app, destination, Some(segment_id.clone()));
        },
    );
}

fn run_export(app: &Rc<RefCell<AppState>>, destination: PathBuf, segment_filter: Option<String>) {
    let result = (|| -> Result<clockbridge::export::ExportReport> {
        let mut state = app.borrow_mut();
        if state.exporting {
            bail!("已有导出任务在进行");
        }
        state.exporting = true;
        let (workspace, events) = {
            let source = state.store.workspace();
            let mut workspace = source.clone();
            if let Some(segment_id) = &segment_filter {
                let segment = source
                    .segment(segment_id)
                    .with_context(|| format!("片段不存在: {segment_id}"))?;
                workspace
                    .segments
                    .retain(|candidate| &candidate.id == segment_id);
                workspace.title = format!("{} · 单段 {}", source.title, segment.label);
            }
            let events = state
                .store
                .events()
                .iter()
                .filter(|event| {
                    segment_filter.is_none()
                        || event.segment_id.is_none()
                        || event.segment_id == segment_filter
                })
                .cloned()
                .collect::<Vec<_>>();
            (workspace, events)
        };
        let report = export_review_bundle(&workspace, &events, &destination, true)?;
        state.exporting = false;
        let _ = state.store.record_event(
            "bundle_exported",
            segment_filter.clone(),
            None,
            "导出可独立复核的证据包",
            vec![
                format!("目录: {}", report.bundle_dir.display()),
                format!("文件数: {}", report.file_count),
                format!("产物副本: {}", report.copied_outputs),
                format!("校验文件: {}", report.checksums_path.display()),
            ],
        );
        Ok(report)
    })();
    match result {
        Ok(report) => {
            show_info(
                app,
                "复核包已导出",
                &format!(
                    "{}\n\n{} 个文件，{} 个产物副本。\n复核命令：cd {} && sha256sum -c SHA256SUMS",
                    report.bundle_dir.display(),
                    report.file_count,
                    report.copied_outputs,
                    report.bundle_dir.display()
                ),
            );
            view::refresh(app);
        }
        Err(error) => {
            if let Ok(mut state) = app.try_borrow_mut() {
                state.exporting = false;
            }
            show_error(app, "导出失败", &format!("{error:#}"));
        }
    }
}

pub(crate) fn show_error(app: &Rc<RefCell<AppState>>, heading: &str, detail: &str) {
    show_dialog(app, heading, detail);
}

fn show_info(app: &Rc<RefCell<AppState>>, heading: &str, detail: &str) {
    show_dialog(app, heading, detail);
}

fn show_dialog(app: &Rc<RefCell<AppState>>, heading: &str, detail: &str) {
    let window = { app.borrow().window.clone() };
    let dialog = gtk::AlertDialog::builder()
        .modal(true)
        .message(heading)
        .detail(detail)
        .build();
    dialog.show(Some(&window));
}

fn sanitize_file_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "segment".to_string()
    } else {
        sanitized
    }
}

fn short_id(id: &str) -> &str {
    id.rsplit('-')
        .next()
        .and_then(|tail| tail.get(..8))
        .unwrap_or(id)
}
