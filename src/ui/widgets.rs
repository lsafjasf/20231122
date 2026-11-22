use gtk::prelude::*;
use gtk4 as gtk;

pub fn hbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, spacing)
}

pub fn vbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, spacing)
}

pub fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    if !class.is_empty() {
        label.add_css_class(class);
    }
    label
}

pub fn wrapped(text: &str, class: &str) -> gtk::Label {
    let label = label(text, class);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label
}

pub fn mono(text: &str, class: &str) -> gtk::Label {
    let label = label(text, class);
    label.add_css_class("mono");
    label
}

pub fn button(text: &str, class: &str) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    if !class.is_empty() {
        button.add_css_class(class);
    }
    button
}

pub fn pill(text: &str, class: &str) -> gtk::Label {
    let pill = gtk::Label::new(Some(text));
    pill.add_css_class("pill");
    if !class.is_empty() {
        pill.add_css_class(class);
    }
    pill
}

pub fn badge(symbol: &str) -> gtk::Label {
    let badge = gtk::Label::new(Some(symbol));
    badge.add_css_class("icon-badge");
    badge.set_halign(gtk::Align::Center);
    badge.set_valign(gtk::Align::Center);
    badge
}

pub fn spacer() -> gtk::Box {
    let spacer = hbox(0);
    spacer.set_hexpand(true);
    spacer
}

pub fn hairline() -> gtk::Box {
    let line = hbox(0);
    line.add_css_class("hairline");
    line.set_hexpand(true);
    line
}

pub fn dot(tone: &str) -> gtk::Box {
    let dot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    dot.add_css_class("status-dot");
    dot.add_css_class(tone);
    dot.set_halign(gtk::Align::Start);
    dot
}

pub fn meta_block(key: &str, value: &str, width: i32) -> gtk::Box {
    let block = vbox(2);
    block.set_width_request(width);
    block.append(&label(key, "meta-key"));
    let value = wrapped(value, "meta-value");
    value.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    block.append(&value);
    block
}

pub fn copy_to_clipboard(text: &str) {
    if let Some(display) = gtk::gdk::Display::default() {
        display.clipboard().set_text(text);
    }
}
