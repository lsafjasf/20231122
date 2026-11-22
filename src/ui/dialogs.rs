use gtk::prelude::*;
use gtk4 as gtk;

use crate::model::Evidence;
use crate::ui::widgets::{button, hairline, hbox, label, pill, spacer, vbox, wrapped};

pub fn show_evidence(parent: &gtk::ApplicationWindow, evidence: &Evidence) {
    let dialog = gtk::Window::builder()
        .title(format!("证据 {}", evidence.id))
        .transient_for(parent)
        .modal(true)
        .default_width(760)
        .default_height(560)
        .build();
    dialog.add_css_class("evidence-dialog");

    let root = vbox(14);
    root.set_margin_top(22);
    root.set_margin_bottom(20);
    root.set_margin_start(24);
    root.set_margin_end(24);
    dialog.set_child(Some(&root));

    let head = hbox(10);
    head.append(&pill(&evidence.id, "pill-info"));
    head.append(&label("EVIDENCE CHAIN", "eyebrow"));
    head.append(&spacer());
    let close = button("关闭", "ghost");
    let dialog_for_close = dialog.clone();
    close.connect_clicked(move |_| dialog_for_close.close());
    head.append(&close);
    root.append(&head);
    root.append(&wrapped(&evidence.title, "dialog-title"));
    root.append(&wrapped(&evidence.prose, "small soft"));
    root.append(&hairline());

    let meta = hbox(24);
    let locator = vbox(3);
    locator.set_hexpand(true);
    locator.append(&label("原始定位", "meta-key"));
    let locator_text = wrapped(&evidence.locator, "mono small cyan");
    locator_text.set_selectable(true);
    locator.append(&locator_text);
    meta.append(&locator);
    let digest = vbox(3);
    digest.set_width_request(320);
    digest.append(&label("摘要 / 依据", "meta-key"));
    let digest_text = wrapped(&evidence.digest, "mono small muted");
    digest_text.set_selectable(true);
    digest_text.set_width_chars(34);
    digest_text.set_max_width_chars(34);
    digest_text.set_wrap_mode(gtk::pango::WrapMode::Char);
    digest.append(&digest_text);
    meta.append(&digest);
    root.append(&meta);

    root.append(&label("保留的原始证据", "meta-key"));
    let view = gtk::TextView::new();
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::Char);
    view.set_top_margin(10);
    view.set_bottom_margin(10);
    view.set_left_margin(12);
    view.set_right_margin(12);
    view.buffer().set_text(&evidence.payload);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroll.set_child(Some(&view));
    scroll.set_vexpand(true);
    scroll.add_css_class("code-view");
    root.append(&scroll);

    let footer = hbox(8);
    footer.append(&spacer());
    let copy = button("复制定位与摘要", "");
    let copy_payload = format!(
        "{}\n{}\n{}\n\n{}",
        evidence.id, evidence.locator, evidence.digest, evidence.payload
    );
    copy.connect_clicked(move |_| crate::ui::widgets::copy_to_clipboard(&copy_payload));
    footer.append(&copy);
    let close = button("完成", "primary");
    let dialog_for_done = dialog.clone();
    close.connect_clicked(move |_| dialog_for_done.close());
    footer.append(&close);
    root.append(&footer);

    dialog.present();
}

pub fn show_text(
    parent: &gtk::ApplicationWindow,
    title: &str,
    kicker: &str,
    body: &str,
    extra: Option<&str>,
) {
    let dialog = gtk::Window::builder()
        .title(title)
        .transient_for(parent)
        .modal(true)
        .default_width(560)
        .build();
    dialog.add_css_class("evidence-dialog");
    let root = vbox(12);
    root.set_margin_top(22);
    root.set_margin_bottom(22);
    root.set_margin_start(24);
    root.set_margin_end(24);
    dialog.set_child(Some(&root));
    root.append(&label(kicker, "eyebrow"));
    root.append(&wrapped(title, "dialog-title"));
    root.append(&wrapped(body, "small soft"));
    if let Some(extra) = extra {
        let extra = wrapped(extra, "mono small muted");
        extra.set_selectable(true);
        root.append(&extra);
    }
    root.append(&hairline());
    let actions = hbox(8);
    actions.append(&spacer());
    let close = button("关闭", "primary");
    let dialog_for_close = dialog.clone();
    close.connect_clicked(move |_| dialog_for_close.close());
    actions.append(&close);
    root.append(&actions);
    dialog.present();
}
