//! Modal dialogs and native file choosers.
//!
//! GTK4 dialogs are callback based; forms collect their entries and invoke a
//! closure on the OK response. File selection uses `FileChooserNative`, which
//! presents the platform chooser and needs no extra permissions.

use gtk::prelude::*;
use gtk::{
    ApplicationWindow, Dialog, DialogFlags, Entry, FileChooserAction, FileChooserNative, Grid,
    Label, ResponseType,
};

/// Minimal modal message dialog.
fn message_dialog(parent: &ApplicationWindow, title: &str, body: &str, is_error: bool) {
    let dialog = Dialog::with_buttons(
        Some(title),
        Some(parent),
        DialogFlags::MODAL | DialogFlags::DESTROY_WITH_PARENT,
        &[("关闭", ResponseType::Close)],
    );
    let label = Label::new(Some(body));
    label.set_margin_top(12);
    label.set_margin_bottom(12);
    label.set_margin_start(16);
    label.set_margin_end(16);
    label.set_wrap(true);
    if is_error {
        label.add_css_class("error");
    }
    dialog.content_area().append(&label);
    dialog.set_transient_for(Some(parent));
    dialog.connect_response(|dialog, _response| dialog.close());
    dialog.present();
}

pub fn show_info(parent: &ApplicationWindow, body: &str) {
    message_dialog(parent, "提示", body, false);
}

pub fn show_error(parent: &ApplicationWindow, body: &str) {
    message_dialog(parent, "错误", body, true);
}

/// Present a labeled entry form. `on_submit` receives the field values only
/// when the user accepts the dialog.
pub fn form_dialog<F: FnOnce(&ApplicationWindow, Vec<String>) + 'static>(
    parent: &ApplicationWindow,
    title: &str,
    fields: &[(&str, &str)],
    on_submit: F,
) {
    let dialog = Dialog::with_buttons(
        Some(title),
        Some(parent),
        DialogFlags::MODAL | DialogFlags::DESTROY_WITH_PARENT,
        &[("取消", ResponseType::Cancel), ("确定", ResponseType::Ok)],
    );
    let grid = Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(8);
    grid.set_margin_top(12);
    grid.set_margin_bottom(12);
    grid.set_margin_start(16);
    grid.set_margin_end(16);
    let mut entries = Vec::new();
    for (row, (label_text, placeholder)) in fields.iter().enumerate() {
        let label = Label::new(Some(label_text));
        label.set_halign(gtk::Align::Start);
        let entry = Entry::new();
        entry.set_placeholder_text(Some(placeholder));
        entry.set_hexpand(true);
        grid.attach(&label, 0, row as i32, 1, 1);
        grid.attach(&entry, 1, row as i32, 2, 1);
        entries.push(entry);
    }
    dialog.content_area().append(&grid);
    dialog.set_transient_for(Some(parent));
    dialog.set_default_response(ResponseType::Ok);
    let weak_parent = gtk::glib::clone::Downgrade::downgrade(parent);
    let on_submit = std::cell::Cell::new(Some(on_submit));
    dialog.connect_response(move |dialog, response| {
        if response == ResponseType::Ok {
            if let Some(parent) = weak_parent.upgrade() {
                let values = entries
                    .iter()
                    .map(|entry| entry.text().to_string())
                    .collect();
                if let Some(callback) = on_submit.take() {
                    callback(&parent, values);
                }
            }
        }
        dialog.close();
    });
    dialog.present();
}

/// Open the native file/folder chooser. `on_select` runs with the chosen path.
pub fn choose_path<F: FnOnce(&ApplicationWindow, std::path::PathBuf) + 'static>(
    parent: &ApplicationWindow,
    title: &str,
    action: FileChooserAction,
    on_select: F,
) {
    let chooser = FileChooserNative::builder()
        .title(title)
        .transient_for(parent)
        .action(action)
        .modal(true)
        .build();
    let weak_parent = gtk::glib::clone::Downgrade::downgrade(parent);
    let on_select = std::cell::Cell::new(Some(on_select));
    chooser.connect_response(move |chooser, response| {
        if response == ResponseType::Accept {
            if let (Some(parent), Some(file)) = (weak_parent.upgrade(), chooser.file()) {
                if let Some(path) = file.path() {
                    if let Some(callback) = on_select.take() {
                        callback(&parent, path);
                    }
                }
            }
        }
        chooser.destroy();
    });
    chooser.show();
}

use crate::state::SharedState;
use crate::widgets::refresh_all_views;

/// Read a string column from a TreeModel row.
fn value_string(model: &gtk::TreeModel, iter: &gtk::TreeIter, column: i32) -> String {
    model
        .get_value(iter, column)
        .get::<Option<String>>()
        .ok()
        .flatten()
        .unwrap_or_default()
}

pub fn show_new_project_dialog(window: &ApplicationWindow, state: SharedState) {
    form_dialog(
        window,
        "新建项目",
        &[
            ("项目名称", "例如 clock-skew-case-7"),
            ("描述", "可选"),
            ("项目目录", "通过下一步选择空目录"),
        ],
        move |window, values| {
            let name = values[0].trim().to_string();
            if name.is_empty() {
                show_error(window, "项目名称不能为空");
                return;
            }
            let description = values[1].clone();
            choose_path(
                window,
                "选择项目目录（需为空或不存在）",
                FileChooserAction::SelectFolder,
                move |window, dir| {
                    let outcome = state.borrow_mut().create_project(
                        dir.clone(),
                        &name,
                        &description,
                        crate::widgets::ACTOR,
                    );
                    match outcome {
                        Ok(()) => {
                            refresh_all_views(&state);
                            show_info(window, &format!("已在 {} 创建项目", dir.display()));
                        }
                        Err(error) => show_error(window, &error.to_string()),
                    }
                },
            );
        },
    );
}

pub fn show_open_project_dialog(window: &ApplicationWindow, state: SharedState) {
    choose_path(
        window,
        "选择包含 project.json 的项目目录",
        FileChooserAction::SelectFolder,
        move |window, dir| match state
            .borrow_mut()
            .open_project(dir.clone(), crate::widgets::ACTOR)
        {
            Ok(()) => {
                refresh_all_views(&state);
                show_info(window, &format!("已重新载入 {}", dir.display()));
            }
            Err(error) => show_error(window, &error.to_string()),
        },
    );
}

pub fn show_add_device_dialog(window: &ApplicationWindow, state: SharedState) {
    form_dialog(
        window,
        "添加设备",
        &[("设备名称", "例如 camera-B"), ("备注", "可选")],
        move |window, values| {
            let label = values[0].trim().to_string();
            if label.is_empty() {
                show_error(window, "设备名称不能为空");
                return;
            }
            let result = match state.borrow_mut().service.as_mut() {
                Some(service) => service
                    .add_device(&label, &values[1])
                    .map_err(anyhow::Error::from),
                None => Err(anyhow::anyhow!("请先打开项目")),
            };
            match result {
                Ok(device) => {
                    refresh_all_views(&state);
                    show_info(
                        window,
                        &format!("已添加设备 {} ({})", device.label, device.id),
                    );
                }
                Err(error) => show_error(window, &error.to_string()),
            }
        },
    );
}

pub fn show_offset_dialog(window: &ApplicationWindow, state: SharedState, tree: &gtk::TreeView) {
    let Some((model, iter)) = tree.selection().selected() else {
        show_error(window, "请先在表格中选择一台设备");
        return;
    };
    let device_id = value_string(&model, &iter, 0);
    let current = value_string(&model, &iter, 2);
    let form_state = state.clone();
    form_dialog(
        window,
        "设置设备时钟偏移",
        &[
            ("偏移 offset_ns", &current),
            ("不确定度 uncertainty_ns", "0"),
        ],
        move |window, values| {
            let offset_ns: i64 = match values[0].trim().parse() {
                Ok(value) => value,
                Err(_) => {
                    show_error(window, "偏移必须是整数纳秒");
                    return;
                }
            };
            let uncertainty_ns: i64 = values[1].trim().parse().unwrap_or(0);
            let result = match form_state.borrow_mut().service.as_mut() {
                Some(service) => service
                    .set_device_offset(&device_id, offset_ns, uncertainty_ns)
                    .map_err(anyhow::Error::from),
                None => Err(anyhow::anyhow!("请先打开项目")),
            };
            match result {
                Ok(()) => {
                    refresh_all_views(&form_state);
                    show_info(window, "时钟偏移已更新并写入审计链");
                }
                Err(error) => show_error(window, &error.to_string()),
            }
        },
    );
}

pub fn show_register_dialog(window: &ApplicationWindow, state: SharedState) {
    let device_options: Vec<String> = state
        .borrow()
        .service
        .as_ref()
        .map(|service| {
            service
                .project
                .devices
                .iter()
                .map(|device| format!("{} ({})", device.id, device.label))
                .collect()
        })
        .unwrap_or_default();
    if device_options.is_empty() {
        show_error(window, "请先在“设备时钟”页添加至少一台设备");
        return;
    }
    form_dialog(
        window,
        "注册媒体片段",
        &[
            ("片段名称", "例如 frag-B-001"),
            ("设备ID", &device_options.join(" 或 ")),
            ("重复对象组ID", "例如 object_7"),
            ("PTS0 设备时钟(Unix ns)", "1700000000000000000"),
            ("模式 interval/keyframe", "interval"),
            ("间隔(ns)", "1000000000"),
            ("缩略图最长边(px)", "480"),
            ("JPEG质量(1-100)", "85"),
        ],
        move |window, values| {
            let label = values[0].trim().to_string();
            let device_id = values[1]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
            let group_id = values[2].trim().to_string();
            let clock_origin: i64 = match values[3].trim().parse() {
                Ok(value) => value,
                Err(_) => {
                    show_error(window, "时钟起点必须是整数 Unix 纳秒");
                    return;
                }
            };
            let config = match build_config_from_values(&values[4..8]) {
                Ok(config) => config,
                Err(error) => {
                    show_error(window, &error.to_string());
                    return;
                }
            };
            let register_state = state.clone();
            let label_for_picker = label.clone();
            choose_path(
                window,
                "选择媒体文件",
                FileChooserAction::Open,
                move |window, source| {
                    let result = match register_state.borrow_mut().service.as_mut() {
                        Some(service) => service
                            .register_segment(
                                &device_id,
                                &label_for_picker,
                                &source,
                                clock_origin,
                                &group_id,
                                config.clone(),
                            )
                            .map_err(anyhow::Error::from),
                        None => Err(anyhow::anyhow!("请先打开项目")),
                    };
                    match result {
                        Ok(segment) => {
                            refresh_all_views(&register_state);
                            show_info(
                                window,
                                &format!(
                                    "片段已注册：{} sha256={}",
                                    segment.id, segment.source_sha256
                                ),
                            );
                        }
                        Err(error) => show_error(window, &error.to_string()),
                    }
                },
            );
        },
    );
}

pub fn show_config_dialog(
    window: &ApplicationWindow,
    state: SharedState,
    selection: &gtk::SingleSelection,
    list: &gtk::StringList,
) {
    let position = selection.selected();
    if position == gtk::INVALID_LIST_POSITION {
        show_error(window, "请先选择一个片段");
        return;
    }
    let Some(segment_text) = list.string(position) else {
        show_error(window, "未选择片段");
        return;
    };
    let segment_id = segment_text
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    let current = state
        .borrow()
        .service
        .as_ref()
        .and_then(|service| service.project.segment(&segment_id))
        .map(|segment| segment.config.clone());
    let Some(current) = current else {
        show_error(window, "未找到该片段");
        return;
    };
    form_dialog(
        window,
        "修改提取配置",
        &[
            ("模式 interval/keyframe", "interval"),
            ("间隔(ns)", &current.interval_ns.to_string()),
            ("缩略图最长边", &current.thumbnail_max_edge.to_string()),
            ("JPEG质量", &current.jpeg_quality.to_string()),
        ],
        move |window, values| match build_config_from_values(&values[0..4]) {
            Ok(config) => {
                let result = match state.borrow_mut().service.as_mut() {
                    Some(service) => service
                        .configure_segment(&segment_id, config)
                        .map_err(anyhow::Error::from),
                    None => Err(anyhow::anyhow!("请先打开项目")),
                };
                match result {
                    Ok(()) => {
                        refresh_all_views(&state);
                        show_info(window, "配置已更新；已提取片段会转为 stale");
                    }
                    Err(error) => show_error(window, &error.to_string()),
                }
            }
            Err(error) => show_error(window, &error.to_string()),
        },
    );
}

pub fn show_export_dialog(window: &ApplicationWindow, state: SharedState) {
    if state.borrow().service.is_none() {
        show_error(window, "请先打开项目");
        return;
    }
    form_dialog(
        window,
        "导出复核包",
        &[("导出包名称", "bundle-1")],
        move |window, values| {
            let bundle = values[0].trim().to_string();
            if bundle.is_empty() {
                show_error(window, "导出包名称不能为空");
                return;
            }
            let export_state = state.clone();
            choose_path(
                window,
                "选择导出根目录",
                FileChooserAction::SelectFolder,
                move |window, export_root| {
                    let result = match export_state.borrow().service.as_ref() {
                        Some(service) => service
                            .export(&export_root, &bundle)
                            .map_err(anyhow::Error::from),
                        None => Err(anyhow::anyhow!("请先打开项目")),
                    };
                    match result {
                        Ok(report) => show_info(
                            window,
                            &format!(
                                "已导出 {} 个文件到 {}\nMANIFEST sha256={}",
                                report.file_count,
                                report.bundle_dir.display(),
                                report.manifest_sha256
                            ),
                        ),
                        Err(error) => show_error(window, &error.to_string()),
                    }
                },
            );
        },
    );
}

fn build_config_from_values(
    values: &[String],
) -> anyhow::Result<vkri_core::model::project::ExtractionConfig> {
    use vkri_core::model::project::KeyframeMode;
    let mode = match values[0].trim().to_ascii_lowercase().as_str() {
        "interval" | "uniform" => KeyframeMode::Interval,
        "keyframe" | "keyframe_only" | "idr" => KeyframeMode::KeyframeOnly,
        other => anyhow::bail!("未知模式: {other}"),
    };
    let interval_ns: i64 = values[1].trim().parse()?;
    let thumbnail_max_edge: u32 = values[2].trim().parse()?;
    let jpeg_quality: u32 = values[3].trim().parse()?;
    anyhow::ensure!(interval_ns > 0, "间隔必须为正");
    anyhow::ensure!(thumbnail_max_edge > 0, "缩略图边长必须为正");
    Ok(vkri_core::model::project::ExtractionConfig {
        mode,
        interval_ns,
        thumbnail_max_edge,
        jpeg_quality: jpeg_quality.clamp(1, 100),
    })
}
