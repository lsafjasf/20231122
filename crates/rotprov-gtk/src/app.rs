use crate::detail;
use gtk4::glib;
use gtk4::prelude::*;
use rotprov_core::model::{Conclusion, InputManifest, RunReport};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

const CSS: &str = r"
.conc-ok     { color: #1a7f37; font-weight: bold; }
.conc-skip   { color: #57606a; font-weight: bold; }
.conc-undec  { color: #9a6700; font-weight: bold; }
.conc-fail   { color: #cf222e; font-weight: bold; }
.trunc-badge { color: #9a6700; font-style: italic; }
.mono        { font-family: monospace; font-size: 0.9em; }
.dim         { color: #57606a; }
.section     { font-weight: bold; margin-top: 12px; }
";

pub fn build(app: &gtk4::Application) {
    let provider = gtk4::CssProvider::new();
    provider.load_from_data(CSS);
    gtk4::style_context_add_provider_for_display(
        &gtk4::gdk::Display::default().expect("需要显示服务器"),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .title("视频旋转元数据修复 · 来源检查")
        .default_width(1080)
        .default_height(720)
        .build();

    // 左：对象列表；右：来源链详情。
    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::Single);
    list.set_width_request(280);
    let list_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&list)
        .build();

    let detail_box = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    detail_box.set_margin_start(16);
    detail_box.set_margin_end(16);
    detail_box.set_margin_top(12);
    detail_box.set_margin_bottom(12);
    let detail_scroll = gtk4::ScrolledWindow::builder()
        .hexpand(true)
        .child(&detail_box)
        .build();

    let paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    paned.set_vexpand(true);
    paned.set_wide_handle(true);
    paned.set_start_child(Some(&list_scroll));
    paned.set_end_child(Some(&detail_scroll));
    paned.set_position(280);

    let status = gtk4::Label::new(Some("正在运行来源检查管线…"));
    status.set_halign(gtk4::Align::Start);
    status.add_css_class("dim");

    let rerun = gtk4::Button::with_label("重新运行");
    let header = gtk4::HeaderBar::new();
    header.pack_end(&rerun);
    window.set_titlebar(Some(&header));

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    root.append(&paned);
    root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    root.append(&status);
    window.set_child(Some(&root));

    let state: Rc<RefCell<Option<RunReport>>> = Rc::new(RefCell::new(None));

    // 选中对象 → 渲染来源链。
    {
        let state = state.clone();
        let detail_box = detail_box.clone();
        list.connect_row_selected(move |_, row| {
            while let Some(child) = detail_box.first_child() {
                detail_box.remove(&child);
            }
            let Some(row) = row else { return };
            let report = state.borrow();
            let Some(report) = report.as_ref() else { return };
            let index = row.index() as usize;
            if let Some(entry) = report.entries.get(index) {
                detail::render_entry(&detail_box, entry);
            }
        });
    }

    // 运行管线（worker 线程），结果回主线程填充。
    let run_pipeline = {
        let state = state.clone();
        let list = list.clone();
        let status = status.clone();
        move || {
            list.select_row(None::<&gtk4::ListBoxRow>);
            while let Some(row) = list.row_at_index(0) {
                list.remove(&row);
            }
            status.set_text("正在运行来源检查管线…");
            let (tx, rx) = std::sync::mpsc::channel::<RunReport>();
            std::thread::spawn(move || {
                let manifests = load_manifests();
                let report = rotprov_core::pipeline::run(&manifests);
                let _ = tx.send(report);
            });
            let state = state.clone();
            let list = list.clone();
            let status = status.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
                match rx.try_recv() {
                    Ok(report) => {
                        status.set_text(&format!(
                            "临时工作区 {} → {}",
                            report.temp_dir,
                            if report.temp_cleaned { "已清理" } else { "未清理!" }
                        ));
                        for entry in &report.entries {
                            list.append(&object_row(entry));
                        }
                        *state.borrow_mut() = Some(report);
                        // ROTFIX_SELECT 用于无头测试时预选某一行。
                        let index = std::env::var("ROTFIX_SELECT")
                            .ok()
                            .and_then(|v| v.parse::<i32>().ok())
                            .unwrap_or(0);
                        list.row_at_index(index).map(|r| list.select_row(Some(&r)));
                        glib::ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                }
            });
        }
    };

    {
        let run_pipeline = run_pipeline.clone();
        rerun.connect_clicked(move |_| run_pipeline());
    }

    run_pipeline();
    window.present();
}

fn load_manifests() -> Vec<InputManifest> {
    let root = std::env::var("ROTFIX_ROOT").unwrap_or_else(|_| ".".to_string());
    let root = PathBuf::from(root);
    let text = std::fs::read_to_string(root.join("fixtures/manifest.json"))
        .expect("无法读取 fixtures/manifest.json");
    let list: Vec<InputManifest> = serde_json::from_str(&text).expect("manifest.json 格式错误");
    list.into_iter()
        .map(|mut m| {
            m.media_path = root.join(&m.media_path).to_string_lossy().to_string();
            m.sidecar_path = root.join(&m.sidecar_path).to_string_lossy().to_string();
            m
        })
        .collect()
}

fn object_row(entry: &rotprov_core::model::ProvenanceEntry) -> gtk4::ListBoxRow {
    let row_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    row_box.set_margin_start(10);
    row_box.set_margin_end(10);
    row_box.set_margin_top(6);
    row_box.set_margin_bottom(6);

    let name = gtk4::Label::new(Some(&entry.manifest.name));
    name.set_halign(gtk4::Align::Start);
    let conc = gtk4::Label::new(Some(&entry.conclusion.label()));
    conc.set_halign(gtk4::Align::Start);
    conc.add_css_class(match &entry.conclusion {
        Conclusion::Fixed { .. } => "conc-ok",
        Conclusion::NoFixNeeded => "conc-skip",
        Conclusion::Undecidable { .. } => "conc-undec",
        Conclusion::Failed { .. } => "conc-fail",
    });
    row_box.append(&name);
    row_box.append(&conc);

    let row = gtk4::ListBoxRow::new();
    row.set_child(Some(&row_box));
    row
}
