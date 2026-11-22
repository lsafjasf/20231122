//! GTK4 来源检查页：把每个结论、证据与处理步骤可视化。

use gtk::prelude::*;
use gtk::{gdk, gio};
use gtk4 as gtk;

use crate::audit::AuditReport;
use crate::model::{Snapshot, Verdict};

fn verdict_css_class(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Aligned => "tag-aligned",
        Verdict::Overlap => "tag-overlap",
        Verdict::Ambiguous => "tag-ambiguous",
        Verdict::Truncated => "tag-truncated",
        Verdict::Malformed => "tag-malformed",
    }
}

fn section_title(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("title-3");
    label.add_css_class("heading");
    label
}

fn mono_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_selectable(true);
    label.add_css_class("monospace");
    label.set_wrap(true);
    label
}

fn snapshot_card(snapshot: &Snapshot) -> gtk::Frame {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 6);
    outer.set_margin_top(6);
    outer.set_margin_bottom(6);
    outer.set_margin_start(10);
    outer.set_margin_end(10);

    let title = gtk::Label::new(Some(&format!(
        "{} — {}（{}）",
        snapshot.id, snapshot.label, snapshot.lang
    )));
    title.set_xalign(0.0);
    title.add_css_class("title-4");
    outer.append(&title);

    outer.append(&mono_label(&format!("SHA-256 : {}", snapshot.sha256)));
    outer.append(&mono_label(&format!(
        "字节数  : {}    行数: {}    记录数: {}",
        snapshot.byte_len,
        snapshot.line_count,
        snapshot.cues.len()
    )));
    outer.append(&mono_label(&format!(
        "临时落盘: {:?}（检查结束后该路径已随工作区删除）",
        snapshot.staged_path
    )));

    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    for cue in &snapshot.cues {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let head = gtk::Label::new(Some(&format!(
            "记录#{}（原始行 {}-{}，序号 {:?}）",
            cue.loc.record_index, cue.loc.line_start, cue.loc.line_end, cue.seq
        )));
        head.set_xalign(0.0);
        head.add_css_class("heading");
        row.append(&head);

        let state = if cue.defects.is_empty() {
            "结构完整".to_string()
        } else {
            format!("缺陷: {}", cue.defects.join("；"))
        };
        let state_label = gtk::Label::new(Some(&state));
        state_label.set_xalign(0.0);
        if !cue.defects.is_empty() {
            state_label.add_css_class("error");
        }
        row.append(&state_label);

        if !cue.text.is_empty() {
            row.append(&mono_label(&format!("字幕正文: {}", cue.text)));
        }
        let raw_preview: String = String::from_utf8_lossy(&cue.raw_record)
            .chars()
            .take(60)
            .collect();
        row.append(&mono_label(&format!("原始字节预览: {raw_preview}")));

        let list_row = gtk::ListBoxRow::new();
        list_row.set_child(Some(&row));
        list.append(&list_row);
    }
    outer.append(&list);

    let frame = gtk::Frame::new(Some(&snapshot.id));
    frame.set_child(Some(&outer));
    frame
}

fn gst_card(title: &str, check: &crate::model::GstCheck) -> gtk::Frame {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 6);
    outer.set_margin_top(6);
    outer.set_margin_bottom(6);
    outer.set_margin_start(10);
    outer.set_margin_end(10);
    outer.append(&mono_label(&format!("pipeline : {}", check.pipeline_desc)));
    outer.append(&mono_label(&format!(
        "成功={}，subparse 吐出字幕 {} 条",
        check.success, check.emitted_cues
    )));
    outer.append(&mono_label(&format!(
        "首帧开始={:?} ns，末帧结束={:?} ns",
        check.first_start_ns, check.last_end_ns
    )));
    outer.append(&mono_label(&format!("bus 消息 : {}", check.bus_message)));
    if let Some(err) = &check.error {
        let err_label = gtk::Label::new(Some(&format!("异常（已隔离，其余对象继续处理）: {err}")));
        err_label.set_xalign(0.0);
        err_label.add_css_class("error");
        err_label.set_wrap(true);
        outer.append(&err_label);
    }
    let frame = gtk::Frame::new(Some(title));
    frame.set_child(Some(&outer));
    frame
}

fn findings_card(report: &AuditReport) -> gtk::Frame {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 6);
    outer.set_margin_top(6);
    outer.set_margin_bottom(6);
    outer.set_margin_start(10);
    outer.set_margin_end(10);

    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    for finding in &report.findings {
        let row_box = gtk::Box::new(gtk::Orientation::Vertical, 4);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let tag = gtk::Label::new(Some(finding.verdict.as_str()));
        tag.add_css_class("tag");
        tag.add_css_class(verdict_css_class(&finding.verdict));
        tag.set_xalign(0.0);
        let reason = gtk::Label::new(Some(&format!("#{} — {}", finding.index, finding.reason)));
        reason.set_xalign(0.0);
        reason.set_wrap(true);
        reason.set_hexpand(true);
        header.append(&tag);
        header.append(&reason);
        row_box.append(&header);

        let pointer = gtk::Label::new(Some(&format!(
            "证据指针 → A 记录 {:?} / B 记录 {:?}（{} 条证据）",
            finding.a_records,
            finding.b_records,
            finding.evidence.len()
        )));
        pointer.set_xalign(0.0);
        pointer.add_css_class("dim-label");
        row_box.append(&pointer);

        for evidence in &finding.evidence {
            row_box.append(&mono_label(&format!("• {evidence}")));
        }

        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&row_box));
        list.append(&row);
    }
    outer.append(&list);

    let frame = gtk::Frame::new(Some("合并结论与证据链"));
    frame.set_child(Some(&outer));
    frame
}

fn steps_card(report: &AuditReport) -> gtk::Frame {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 4);
    outer.set_margin_top(6);
    outer.set_margin_bottom(6);
    outer.set_margin_start(10);
    outer.set_margin_end(10);
    for step in &report.steps {
        let mark = if step.ok { "✓" } else { "✗" };
        let label = gtk::Label::new(Some(&format!(
            "{mark} {:>2}. {} / {} — {}",
            step.seq, step.stage, step.target, step.detail
        )));
        label.set_xalign(0.0);
        label.set_wrap(true);
        if !step.ok {
            label.add_css_class("error");
        }
        outer.append(&label);
    }
    let frame = gtk::Frame::new(Some("处理步骤（可回溯审计链）"));
    frame.set_child(Some(&outer));
    frame
}

fn cleanup_card(report: &AuditReport) -> gtk::Frame {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 4);
    outer.set_margin_top(6);
    outer.set_margin_bottom(6);
    outer.set_margin_start(10);
    outer.set_margin_end(10);
    outer.append(&mono_label(&format!(
        "临时目录: {:?}",
        report.temp_root_created
    )));
    outer.append(&mono_label(&format!(
        "清理前存在: {}    清理后存在: {}",
        report.temp_root_existed_before_cleanup, report.temp_root_exists_after_cleanup
    )));
    outer.append(&mono_label(&format!("持久报告: {:?}", report.report_path)));
    let frame = gtk::Frame::new(Some("临时状态清理"));
    frame.set_child(Some(&outer));
    frame
}

const CSS: &str = r#"
.tag {
  padding: 2px 10px;
  border-radius: 9999px;
  font-weight: bold;
  color: #ffffff;
}
.tag-aligned { background-color: #26743b; }
.tag-overlap { background-color: #a0681c; }
.tag-ambiguous { background-color: #8a3ffc; }
.tag-truncated { background-color: #b54708; }
.tag-malformed { background-color: #a51c30; }
.monospace { font-family: monospace; font-size: 11px; }
.error { color: #c01c28; }
"#;

/// 启动 GTK4 来源检查页。
pub fn run_window(report: AuditReport) {
    if let Err(err) = gtk::init() {
        eprintln!("GTK4 初始化失败（当前环境可能无显示服务）: {err}");
        eprintln!("可改用 `--selftest` 在无界面模式运行完整检查。");
        std::process::exit(3);
    }
    let app = gtk::Application::new(
        Some("org.submerge.sourcecheck"),
        gio::ApplicationFlags::empty(),
    );

    // AuditReport 在应用生命周期内通过 Rc 共享，shutdown 时随最后一个引用释放。
    let report = std::rc::Rc::new(report);
    let report_for_activate = std::rc::Rc::clone(&report);
    app.connect_activate(move |app| {
        let report: &AuditReport = &report_for_activate;
        let provider = gtk::CssProvider::new();
        provider.load_from_data(CSS);
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.set_margin_top(14);
        root.set_margin_bottom(14);
        root.set_margin_start(16);
        root.set_margin_end(16);

        let header = gtk::Label::new(Some("多语言字幕时间轴合并 · 来源检查页"));
        header.add_css_class("title-2");
        header.set_xalign(0.0);
        root.append(&header);
        let meta = gtk::Label::new(Some(&format!(
            "生成时间 {} · 技术栈 Rust + GStreamer {} + GTK4 {}",
            report.started_at,
            gstreamer::version_string(),
            gtk::major_version()
        )));
        meta.set_xalign(0.0);
        meta.add_css_class("dim-label");
        root.append(&meta);

        root.append(&section_title("版本快照（两份原始输入 + SHA-256）"));
        let snaps = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        snaps.append(&snapshot_card(&report.snapshot_a));
        snaps.append(&snapshot_card(&report.snapshot_b));
        root.append(&snaps);

        root.append(&section_title("GStreamer 独立核验（异常隔离）"));
        let gst_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        gst_box.append(&gst_card("snapshot_a", &report.gst_a));
        gst_box.append(&gst_card("snapshot_b", &report.gst_b));
        root.append(&gst_box);

        root.append(&section_title("合并判定"));
        root.append(&findings_card(report));

        root.append(&section_title("审计与清理"));
        root.append(&steps_card(report));
        root.append(&cleanup_card(report));

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_child(Some(&root));
        scroller.set_vexpand(true);

        let window = gtk::ApplicationWindow::new(app);
        window.set_title(Some("来源检查 — 字幕时间轴合并"));
        window.set_default_size(1120, 820);
        window.set_child(Some(&scroller));
        window.present();
    });

    app.connect_shutdown(move |_| {
        drop(std::rc::Rc::clone(&report));
    });

    let empty: Vec<String> = Vec::new();
    app.run_with_args(&empty);
}

/// `--selftest` 模式：在无显示环境下打印流水线摘要，并以退出码反映硬性约束。
pub fn print_selftest_summary(report: &AuditReport) {
    println!("\n===== 自检摘要 =====");
    println!(
        "快照 A: {} 条记录，SHA-256={}",
        report.snapshot_a.cues.len(),
        report.snapshot_a.sha256
    );
    println!(
        "快照 B: {} 条记录，SHA-256={}",
        report.snapshot_b.cues.len(),
        report.snapshot_b.sha256
    );
    println!(
        "GST 核验: A.success={} B.success={}",
        report.gst_a.success, report.gst_b.success
    );
    for finding in &report.findings {
        println!(
            "  结论#{} {:?} — {}（证据 {} 条）",
            finding.index,
            finding.verdict,
            finding.reason,
            finding.evidence.len()
        );
    }
    println!("处理步骤 {} 条", report.steps.len());
    println!(
        "临时目录清理: before={} after={}",
        report.temp_root_existed_before_cleanup, report.temp_root_exists_after_cleanup
    );

    let mut failures = Vec::new();
    if report.temp_root_exists_after_cleanup {
        failures.push("临时工作区未被清理");
    }
    if !report
        .findings
        .iter()
        .any(|f| f.verdict == Verdict::Ambiguous)
    {
        failures.push("缺少无法判断的边界结论");
    }
    if !report
        .findings
        .iter()
        .any(|f| f.verdict == Verdict::Truncated)
    {
        failures.push("缺少截断记录结论");
    }
    if !report.gst_a.success || !report.gst_b.success {
        failures.push("存在 GStreamer 核验失败");
    }
    if failures.is_empty() {
        println!("自检结果: 全部硬性约束通过 ✓");
    } else {
        for failure in &failures {
            println!("自检失败: {failure}");
        }
        std::process::exit(2);
    }
}
