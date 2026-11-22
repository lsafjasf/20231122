//! GTK4 front-end: file chooser, box table, error offset, repair button and
//! GStreamer reload results.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::gst_check;
use crate::isobmff::{inspect_file, Report};
use crate::repair;
use crate::report_view;

#[derive(Default)]
struct AppState {
    path: Option<PathBuf>,
    report: Option<Report>,
}

pub fn run() -> anyhow::Result<()> {
    let app = gtk::Application::new(
        Some("org.example.mp4integrity"),
        gio::ApplicationFlags::HANDLES_OPEN,
    );
    app.connect_activate(build_ui);
    app.connect_open(|app, _files, _hint| {
        app.activate();
    });
    let exit_code = app.run();
    if exit_code == glib::ExitCode::SUCCESS {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "GTK application exited with {:?}",
            exit_code
        ))
    }
}

/// Modal open/save dialog built on GtkFileChooserDialog (the binding available
/// in this environment does not expose GtkFileDialog).
fn run_file_chooser(
    parent: &gtk::ApplicationWindow,
    title: &str,
    action: gtk::FileChooserAction,
    initial_name: Option<&str>,
    on_pick: impl Fn(PathBuf) + 'static,
) {
    let dialog = gtk::FileChooserDialog::builder()
        .title(title)
        .action(action)
        .transient_for(parent)
        .modal(true)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    dialog.add_button("Select", gtk::ResponseType::Accept);
    if let Some(name) = initial_name {
        dialog.set_current_name(name);
    }
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            if let Some(file) = dialog.file() {
                if let Some(path) = file.path() {
                    on_pick(path);
                }
            }
        }
        dialog.destroy();
    });
    dialog.present();
}

fn build_ui(app: &gtk::Application) {
    let state = Rc::new(RefCell::new(AppState::default()));

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("MP4 Container Integrity Checker")
        .default_width(1080)
        .default_height(720)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_margin_start(10);
    root.set_margin_end(10);

    // Header row: file picker + repair + reload.
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let open_btn = gtk::Button::with_label("Open container…");
    let repair_btn = gtk::Button::with_label("Repair into new file…");
    let reload_btn = gtk::Button::with_label("Reload via GStreamer");
    repair_btn.set_sensitive(false);
    reload_btn.set_sensitive(false);
    let file_label = gtk::Label::new(Some("no file selected"));
    file_label.set_hexpand(true);
    file_label.set_halign(gtk::Align::Start);
    controls.append(&open_btn);
    controls.append(&repair_btn);
    controls.append(&reload_btn);
    controls.append(&file_label);
    root.append(&controls);

    // Summary line.
    let summary = gtk::Label::new(None);
    summary.set_halign(gtk::Align::Start);
    summary.set_margin_top(6);
    root.append(&summary);

    // Box table.
    let model = gtk::StringList::new(&[]);
    let selection = gtk::NoSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, list_item| {
        let label = gtk::Label::new(None);
        label.set_halign(gtk::Align::Start);
        label.set_selectable(true);
        list_item.set_child(Some(&label));
    });
    factory.connect_bind(|_, list_item| {
        let item = list_item
            .item()
            .and_downcast::<gtk::StringObject>()
            .expect("string object");
        let label = list_item
            .child()
            .and_downcast::<gtk::Label>()
            .expect("label");
        label.set_text(&item.string());
    });
    let list_view = gtk::ListView::new(Some(selection), Some(factory));
    let scroller = gtk::ScrolledWindow::builder()
        .child(&list_view)
        .vexpand(true)
        .build();
    root.append(&scroller);

    // Error / advice panel.
    let error_view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .height_request(160)
        .build();
    root.append(&error_view);

    // GStreamer reload result panel.
    let reload_view = gtk::Label::new(Some("GStreamer reload: not run yet"));
    reload_view.set_halign(gtk::Align::Start);
    reload_view.set_margin_top(4);
    root.append(&reload_view);

    window.set_child(Some(&root));

    let state_open = Rc::clone(&state);
    let model_open = model.clone();
    let summary_open = summary.clone();
    let error_open = error_view.clone();
    let repair_open = repair_btn.clone();
    let reload_open = reload_btn.clone();
    let file_label_open = file_label.clone();
    let window_open = window.clone();
    open_btn.connect_clicked(move |_| {
        let model = model_open.clone();
        let summary = summary_open.clone();
        let error_view = error_open.clone();
        let repair_btn = repair_open.clone();
        let reload_btn = reload_open.clone();
        let file_label = file_label_open.clone();
        let state = Rc::clone(&state_open);
        run_file_chooser(
            &window_open,
            "Choose MP4 container",
            gtk::FileChooserAction::Open,
            None,
            move |path| {
                let report = match inspect_file(&path) {
                    Ok(r) => r,
                    Err(e) => {
                        error_view
                            .buffer()
                            .set_text(&format!("failed to read file: {e}"));
                        return;
                    }
                };
                populate_model(&model, &report);
                summary
                    .set_text(&report_view::render_summary(&report, &path).replace('\n', "  |  "));
                error_view
                    .buffer()
                    .set_text(&report_view::render_errors(&report));
                file_label.set_text(&path.display().to_string());
                repair_btn.set_sensitive(!report.is_healthy());
                reload_btn.set_sensitive(true);
                *state.borrow_mut() = AppState {
                    path: Some(path),
                    report: Some(report),
                };
            },
        );
    });

    let state_repair = Rc::clone(&state);
    let error_repair = error_view.clone();
    let window_repair = window.clone();
    repair_btn.connect_clicked(move |_| {
        let state = state_repair.borrow();
        let (Some(input), Some(report)) = (state.path.clone(), state.report.clone()) else {
            return;
        };
        drop(state);
        let error_view = error_repair.clone();
        run_file_chooser(
            &window_repair,
            "Save repaired container as (original will not be overwritten)",
            gtk::FileChooserAction::Save,
            Some("repaired.mp4"),
            move |out| match repair::repair_file(&input, &out) {
                Ok(outcome) => {
                    let text = format!(
                        "repair OK -> {}\nbytes written: {}\ndropped boxes: {}\ntrimmed tail: {}\n\n{}",
                        outcome.output.display(),
                        outcome.bytes_written,
                        outcome.dropped_boxes.join(", "),
                        outcome.trimmed_tail,
                        report_view::render_errors(&report)
                    );
                    error_view.buffer().set_text(&text);
                }
                Err(e) => error_view.buffer().set_text(&format!("repair refused: {e}")),
            },
        );
    });

    let state_reload = Rc::clone(&state);
    reload_btn.connect_clicked(move |_| {
        let path = state_reload.borrow().path.clone();
        let Some(path) = path else { return };
        match gst_check::reload_and_count(&path) {
            Ok(result) => reload_view.set_text(&format!(
                "GStreamer reload: success={}  tracks after reload: {}  ({})",
                result.success,
                result.track_count,
                result.track_kinds.join(", ")
            )),
            Err(e) => reload_view.set_text(&format!("GStreamer reload failed: {e}")),
        }
    });

    window.present();
}

fn populate_model(model: &gtk::StringList, report: &Report) {
    while model.n_items() > 0 {
        model.remove(0);
    }
    let header = format!(
        "{:>10}  {:<9}  {:>10}  {:<18}  {:<11}  {}",
        "offset", "type", "size", "owner", "status", "repairability"
    );
    model.append(&header);
    for b in &report.boxes {
        let indent = "  ".repeat(b.depth);
        let row = format!(
            "{:>10}  {}{:<8}  {:>10}  {:<18}  {:<11}  {}",
            b.offset,
            indent,
            b.box_type,
            b.declared_size,
            if b.owner.is_empty() { "file" } else { &b.owner },
            report_view::kind_label(b.error),
            b.repair.label()
        );
        model.append(&row);
    }
}
