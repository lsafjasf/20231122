use std::time::Duration;

use gtk::prelude::*;
use gtk::{Align, Box, Button, Label, Orientation, Revealer};

pub fn vbox(spacing: i32) -> Box {
    Box::new(Orientation::Vertical, spacing)
}

pub fn hbox(spacing: i32) -> Box {
    Box::new(Orientation::Horizontal, spacing)
}

pub fn label_with(text: &str, classes: &[&str]) -> Label {
    let widget = Label::new(Some(text));
    widget.set_xalign(0.0);
    for class in classes {
        widget.add_css_class(class);
    }
    widget
}

pub fn wrapped(text: &str, classes: &[&str]) -> Label {
    let widget = label_with(text, classes);
    widget.set_wrap(true);
    widget.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    widget
}

pub fn mono(text: &str, classes: &[&str]) -> Label {
    let mut all = vec!["mono"];
    all.extend_from_slice(classes);
    label_with(text, &all)
}

pub fn pill(text: &str, class: &str) -> Label {
    let widget = Label::new(Some(text));
    widget.add_css_class("status-pill");
    widget.add_css_class(class);
    widget
}

pub fn panel(spacing: i32) -> Box {
    let widget = vbox(spacing);
    widget.add_css_class("panel");
    widget
}

pub fn rule() -> Box {
    let widget = Box::new(Orientation::Vertical, 0);
    widget.add_css_class("divider");
    widget.set_height_request(1);
    widget
}

pub fn hspacer() -> Box {
    let widget = Box::new(Orientation::Horizontal, 0);
    widget.set_hexpand(true);
    widget
}

pub fn vspacer() -> Box {
    let widget = Box::new(Orientation::Vertical, 0);
    widget.set_vexpand(true);
    widget
}

pub fn text_button(text: &str, class: &str) -> Button {
    let widget = Button::with_label(text);
    widget.add_css_class(class);
    widget
}

pub fn glyph_button(glyph: &str, tooltip: &str) -> Button {
    let widget = Button::with_label(glyph);
    widget.add_css_class("top-icon");
    widget.set_tooltip_text(Some(tooltip));
    widget
}

pub fn icon_text(glyph: &str, text: &str, glyph_class: &str, text_class: &str) -> Box {
    let row = hbox(7);
    let glyph = label_with(glyph, &[glyph_class]);
    let text = label_with(text, &[text_class]);
    row.append(&glyph);
    row.append(&text);
    row
}

pub fn toast(overlay: &gtk::Overlay) -> Toast {
    let label = Label::new(None);
    label.add_css_class("toast");
    label.set_halign(Align::Center);
    label.set_valign(Align::End);
    label.set_margin_bottom(24);
    let revealer = Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideUp);
    revealer.set_transition_duration(180);
    revealer.set_child(Some(&label));
    revealer.set_halign(Align::Center);
    revealer.set_valign(Align::End);
    revealer.set_margin_bottom(18);
    revealer.set_reveal_child(false);
    overlay.add_overlay(&revealer);
    Toast { label, revealer }
}

#[derive(Clone)]
pub struct Toast {
    label: Label,
    revealer: Revealer,
}

impl Toast {
    pub fn show(&self, message: &str) {
        self.label.set_text(message);
        self.revealer.set_reveal_child(true);
        let revealer = self.revealer.clone();
        gtk::glib::timeout_add_local_once(Duration::from_millis(2_600), move || {
            revealer.set_reveal_child(false);
        });
    }
}

pub fn copy_to_clipboard(text: &str) {
    if let Some(display) = gtk::gdk::Display::default() {
        display.clipboard().set_text(text);
    }
}
