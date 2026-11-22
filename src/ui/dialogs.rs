use gtk::Align;
use gtk::prelude::*;

use crate::model::Session;
use crate::ui::components::{
    Toast, copy_to_clipboard, hbox, hspacer, label_with, mono, rule, text_button, vbox, wrapped,
};

pub fn show_evidence_dialog(
    parent: &gtk::Window,
    session: &Session,
    toast: &Toast,
    evidence_id: &str,
) {
    let Some(evidence) = session.evidence(evidence_id) else {
        toast.show("找不到该证据记录");
        return;
    };
    let window = gtk::Window::new();
    window.set_title(Some("证据详情 · Backpressure Inspector"));
    window.set_transient_for(Some(parent));
    window.set_modal(true);
    window.set_default_size(680, 520);
    window.set_resizable(true);

    let content = vbox(14);
    content.add_css_class("dialog");
    content.set_margin_top(20);
    content.set_margin_bottom(20);
    content.set_margin_start(20);
    content.set_margin_end(20);

    let top = hbox(8);
    top.append(&label_with("EVIDENCE TRACE", &["eyebrow"]));
    top.append(&mono(&evidence.id, &["xs", "dim"]));
    top.append(&hspacer());
    let close = text_button("关闭", "btn-ghost");
    let window_for_close = window.clone();
    close.connect_clicked(move |_| window_for_close.close());
    top.append(&close);
    content.append(&top);

    let title = wrapped(&evidence.title, &["dialog-title"]);
    content.append(&title);
    content.append(&wrapped(
        &format!("{} · 捕获于 {}", evidence.kind, evidence.captured),
        &["dialog-body"],
    ));
    content.append(&rule());

    content.append(&trace_row("原始输入", &evidence.source_ref, session));
    content.append(&trace_row("处理步骤", &evidence.step_ref, session));
    content.append(&rule());

    let payload_header = hbox(8);
    payload_header.append(&label_with("原始证据载荷", &["panel-title"]));
    payload_header.append(&hspacer());
    payload_header.append(&mono("READ ONLY", &["xs", "dim"]));
    content.append(&payload_header);

    let payload = wrapped(&evidence.payload, &["payload"]);
    payload.set_selectable(true);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroll.set_child(Some(&payload));
    scroll.set_vexpand(true);
    scroll.set_min_content_height(220);
    content.append(&scroll);

    let actions = hbox(8);
    actions.set_halign(Align::End);
    let copy = text_button("复制原始内容", "btn-ghost");
    let toast_clone = toast.clone();
    let text = evidence.payload.clone();
    copy.connect_clicked(move |_| {
        copy_to_clipboard(&text);
        toast_clone.show("原始证据已复制到剪贴板");
    });
    let open_source = text_button("查看来源上下文", "btn-ghost");
    let parent_clone = parent.clone();
    let session_clone = session.clone();
    let toast_clone = toast.clone();
    let source_ref = evidence.source_ref.clone();
    open_source.connect_clicked(move |_| {
        show_trace_dialog(&parent_clone, &session_clone, &source_ref);
        toast_clone.show("已定位到来源上下文");
    });
    actions.append(&open_source);
    actions.append(&copy);
    content.append(&actions);

    window.set_child(Some(&content));
    window.present();
}

fn trace_row(caption: &str, reference: &str, session: &Session) -> gtk::Box {
    let row = hbox(10);
    let caption = label_with(caption, &["source-key"]);
    caption.set_width_chars(9);
    row.append(&caption);

    if let Some(step) = session.step(reference) {
        row.append(&mono(reference, &["xs", "status-good", "status-pill"]));
        row.append(&label_with(
            &format!("{} · {} · {}", step.time, step.name, step.element),
            &["source-val"],
        ));
    } else if reference.starts_with("IN-") {
        row.append(&mono(reference, &["xs", "status-good", "status-pill"]));
        row.append(&mono(&session.source.uri, &["xs", "source-uri"]));
    } else {
        row.append(&mono(reference, &["xs", "muted"]));
    }
    row
}

pub fn show_trace_dialog(parent: &gtk::Window, session: &Session, reference: &str) {
    let window = gtk::Window::new();
    window.set_title(Some("来源上下文 · Backpressure Inspector"));
    window.set_transient_for(Some(parent));
    window.set_modal(true);
    window.set_default_size(620, 420);

    let content = vbox(12);
    content.add_css_class("dialog");
    content.set_margin_top(20);
    content.set_margin_bottom(20);
    content.set_margin_start(20);
    content.set_margin_end(20);

    content.append(&label_with("SOURCE CONTEXT", &["eyebrow"]));
    content.append(&wrapped("原始输入", &["dialog-title"]));
    content.append(&mono(&session.source.uri, &["source-uri"]));
    content.append(&rule());
    for (key, value) in [
        ("格式", session.source.format.as_str()),
        ("视频", session.source.video.as_str()),
        ("音频", session.source.audio.as_str()),
        ("指纹", session.source.fingerprint.as_str()),
        ("探测窗口", session.source.probed.as_str()),
    ] {
        let row = hbox(8);
        let key = label_with(key, &["source-key"]);
        key.set_width_chars(8);
        row.append(&key);
        row.append(&wrapped(value, &["source-val"]));
        content.append(&row);
    }
    content.append(&rule());
    content.append(&label_with(
        &format!("引用 {reference} 的结论均由此输入与相邻处理步骤生成。"),
        &["dialog-body"],
    ));
    let close = text_button("关闭", "btn-ghost");
    close.set_halign(Align::End);
    let window_clone = window.clone();
    close.connect_clicked(move |_| window_clone.close());
    content.append(&close);
    window.set_child(Some(&content));
    window.present();
}

pub fn show_message_dialog(
    parent: &gtk::Window,
    title: &str,
    body: &str,
    detail: Option<&str>,
    command: Option<&str>,
    toast: &Toast,
) {
    let window = gtk::Window::new();
    window.set_title(Some(title));
    window.set_transient_for(Some(parent));
    window.set_modal(true);
    window.set_default_size(640, if command.is_some() { 330 } else { 250 });

    let content = vbox(12);
    content.add_css_class("dialog");
    content.set_margin_top(20);
    content.set_margin_bottom(20);
    content.set_margin_start(20);
    content.set_margin_end(20);
    content.append(&label_with("ACTION RESULT", &["eyebrow"]));
    content.append(&wrapped(title, &["dialog-title"]));
    content.append(&wrapped(body, &["dialog-body"]));
    if let Some(detail) = detail {
        content.append(&wrapped(detail, &["failure-text"]));
    }
    if let Some(command) = command {
        let command_label = mono(command, &["payload"]);
        command_label.set_wrap(true);
        command_label.set_selectable(true);
        content.append(&command_label);
        let copy = text_button("复制诊断命令", "btn-ghost");
        copy.set_halign(Align::Start);
        let toast_clone = toast.clone();
        let command = command.to_string();
        copy.connect_clicked(move |_| {
            copy_to_clipboard(&command);
            toast_clone.show("诊断命令已复制");
        });
        content.append(&copy);
    }
    let actions = hbox(8);
    actions.set_halign(Align::End);
    let close = text_button("关闭", "btn-primary");
    let window_clone = window.clone();
    close.connect_clicked(move |_| window_clone.close());
    actions.append(&close);
    content.append(&actions);
    window.set_child(Some(&content));
    window.present();
}
