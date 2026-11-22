//! GTK4 graphical interface.
//!
//! Design points mapped to the workflow:
//! - the fragment list is the single source of truth and keeps on-disk order;
//! - each row has its own Run/Rerun button so fragments are independent;
//! - processing happens on a worker thread; the UI only receives channel
//!   messages, so the window stays responsive and can be closed any time;
//! - every transition is saved immediately, so quitting and reopening
//!   restores the exact state and evidence.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};

use anyhow::Result;
use gtk4::glib;
use gtk4::prelude::*;

use crate::engine;
use crate::model::{Fragment, FragmentStatus, Project};
use crate::review;
use crate::storage;
use crate::util::now_rfc3339;

#[derive(Clone)]
struct AppState {
    project_dir: Rc<RefCell<Option<PathBuf>>>,
    project: Rc<RefCell<Option<Project>>>,
    busy: Rc<RefCell<bool>>,
}

enum WorkerMsg {
    Finished { id: String, result: Result<crate::model::RunEvidence, String> },
}

pub fn run() -> Result<()> {
    crate::media::init()?;
    let app = gtk4::Application::new(
        Some("org.srcrepair.Tool"),
        gtk4::gio::ApplicationFlags::FLAGS_NONE,
    );
    app.connect_activate(build_ui);
    let empty: Vec<&str> = Vec::new();
    app.run_with_args(&empty);
    Ok(())
}

fn build_ui(app: &gtk4::Application) {
    let state = AppState {
        project_dir: Rc::new(RefCell::new(None)),
        project: Rc::new(RefCell::new(None)),
        busy: Rc::new(RefCell::new(false)),
    };

    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .title("srcrepair — sample-rate conversion repair")
        .default_width(1080)
        .default_height(720)
        .build();

    let header = gtk4::HeaderBar::new();
    window.set_titlebar(Some(&header));

    let new_btn = gtk4::Button::with_label("New project");
    let open_btn = gtk4::Button::with_label("Open project");
    let export_btn = gtk4::Button::with_label("Export review bundle");
    header.pack_start(&new_btn);
    header.pack_start(&open_btn);
    header.pack_end(&export_btn);

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    root.set_margin_top(8);
    root.set_margin_bottom(8);
    root.set_margin_start(8);
    root.set_margin_end(8);
    window.set_child(Some(&root));

    let info_bar = gtk4::Label::new(Some(
        "No project open — create or open one to register media fragments.",
    ));
    info_bar.set_halign(gtk4::Align::Start);
    info_bar.add_css_class("dim-label");
    root.append(&info_bar);

    let add_form = construct_add_form();
    add_form.frame.set_sensitive(false);
    root.append(&add_form.frame);

    let list_box = gtk4::ListBox::new();
    list_box.set_selection_mode(gtk4::SelectionMode::Single);
    let scrolled = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .min_content_height(320)
        .child(&list_box)
        .build();
    root.append(&scrolled);

    let detail = gtk4::TextView::builder()
        .editable(false)
        .monospace(true)
        .height_request(180)
        .build();
    let detail_scroll = gtk4::ScrolledWindow::builder()
        .vexpand(false)
        .child(&detail)
        .build();
    root.append(&detail_scroll);

    let status_label = gtk4::Label::new(None);
    status_label.set_halign(gtk4::Align::Start);
    status_label.add_css_class("dim-label");
    root.append(&status_label);

    let ctx = Rc::new(UiCtx {
        window: window.clone(),
        list_box: list_box.clone(),
        detail: detail.clone(),
        info_bar: info_bar.clone(),
        status_label: status_label.clone(),
        add_frame: add_form.frame.clone(),
        export_btn: export_btn.clone(),
        state: state.clone(),
    });

    wire_new_button(&new_btn, &ctx);
    wire_open_button(&open_btn, &ctx);
    wire_export_button(&export_btn, &ctx);
    wire_add_form(&add_form, &ctx);
    wire_selection(&list_box, &ctx);
    // Render the initial (empty) state; per-run channels are attached in
    // start_worker whenever a fragment is launched.
    refresh_all(&ctx);

    window.present();
}

struct UiCtx {
    window: gtk4::ApplicationWindow,
    list_box: gtk4::ListBox,
    detail: gtk4::TextView,
    info_bar: gtk4::Label,
    status_label: gtk4::Label,
    add_frame: gtk4::Frame,
    export_btn: gtk4::Button,
    state: AppState,
}

/// Form fields for registering a new fragment.
struct AddForm {
    frame: gtk4::Frame,
    device: gtk4::Entry,
    source: gtk4::Entry,
    source_rate: gtk4::Entry,
    gap_start: gtk4::Entry,
    gap_duration: gtk4::Entry,
    gap_reason: gtk4::Entry,
    browse: gtk4::Button,
    submit: gtk4::Button,
}

fn construct_add_form() -> AddForm {
    let frame = gtk4::Frame::new(Some("Register fragment (missing-segment gap)"));
    let grid = gtk4::Grid::new();
    grid.set_column_spacing(8);
    grid.set_row_spacing(6);
    grid.set_margin_top(8);
    grid.set_margin_bottom(8);
    grid.set_margin_start(8);
    grid.set_margin_end(8);
    frame.set_child(Some(&grid));

    let device = gtk4::Entry::new();
    device.set_placeholder_text(Some("device label, e.g. recorder-A"));
    let source = gtk4::Entry::new();
    source.set_hexpand(true);
    source.set_placeholder_text(Some("path to source media"));
    let browse = gtk4::Button::with_label("Browse…");
    let source_rate = gtk4::Entry::new();
    source_rate.set_placeholder_text(Some("source rate Hz, e.g. 44100"));
    let gap_start = gtk4::Entry::new();
    gap_start.set_placeholder_text(Some("gap start ns"));
    gap_start.set_text("0");
    let gap_duration = gtk4::Entry::new();
    gap_duration.set_placeholder_text(Some("gap duration ns (0 = none)"));
    gap_duration.set_text("0");
    let gap_reason = gtk4::Entry::new();
    gap_reason.set_hexpand(true);
    gap_reason.set_placeholder_text(Some("why this segment is missing"));
    let submit = gtk4::Button::with_label("Add fragment");
    submit.add_css_class("suggested-action");

    let items: [(&str, &gtk4::Widget); 7] = [
        ("Device", device.upcast_ref()),
        ("Source file", source.upcast_ref()),
        ("Source rate", source_rate.upcast_ref()),
        ("Gap start (ns)", gap_start.upcast_ref()),
        ("Gap duration (ns)", gap_duration.upcast_ref()),
        ("Gap reason", gap_reason.upcast_ref()),
        ("", submit.upcast_ref()),
    ];
    for (row, (label, widget)) in items.iter().enumerate() {
        grid.attach(&gtk4::Label::new(Some(label)), 0, row as i32, 1, 1);
        grid.attach(*widget, 1, row as i32, 2, 1);
    }
    grid.attach(&browse, 3, 1, 1, 1);

    AddForm {
        frame,
        device,
        source,
        source_rate,
        gap_start,
        gap_duration,
        gap_reason,
        browse,
        submit,
    }
}

fn wire_new_button(btn: &gtk4::Button, ctx: &Rc<UiCtx>) {
    let ctx = ctx.clone();
    btn.connect_clicked(move |_| {
        let dialog = gtk4::Dialog::new();
        dialog.set_title(Some("New project — choose an empty/new directory"));
        dialog.set_transient_for(Some(&ctx.window));
        dialog.set_modal(true);
        let chooser = gtk4::FileChooserWidget::new(gtk4::FileChooserAction::SelectFolder);
        dialog.set_child(Some(&chooser));
        let rate_entry = gtk4::Entry::new();
        rate_entry.set_text("48000");
        let bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        bar.append(&gtk4::Label::new(Some("Target rate Hz:")));
        bar.append(&rate_entry);
        let area = dialog.content_area();
        area.append(&bar);
        dialog.add_button("Cancel", gtk4::ResponseType::Cancel);
        dialog.add_button("Create", gtk4::ResponseType::Ok);
        let ctx2 = ctx.clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk4::ResponseType::Ok {
                if let Some(folder) = chooser.file() {
                    if let Some(path) = folder.path() {
                        let rate: u32 = rate_entry.text().parse().unwrap_or(48000);
                        create_and_load(&ctx2, &path, rate);
                    }
                }
            }
            dialog.close();
        });
        dialog.present();
    });
}

fn create_and_load(ctx: &Rc<UiCtx>, dir: &std::path::Path, target_rate: u32) {
    let id = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "srcrepair-project".into());
    let project = Project::new(id, target_rate, now_rfc3339());
    if let Err(err) = storage::save_project(dir, &project) {
        show_error(ctx, &format!("cannot create project: {err:#}"));
        return;
    }
    load_into(ctx, dir.to_path_buf(), project);
}

fn wire_open_button(btn: &gtk4::Button, ctx: &Rc<UiCtx>) {
    let ctx = ctx.clone();
    btn.connect_clicked(move |_| {
        let dialog = gtk4::FileChooserDialog::new(
            Some("Open project directory"),
            Some(&ctx.window),
            gtk4::FileChooserAction::SelectFolder,
            &[
                ("Cancel", gtk4::ResponseType::Cancel),
                ("Open", gtk4::ResponseType::Accept),
            ],
        );
        let ctx2 = ctx.clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk4::ResponseType::Accept {
                if let Some(folder) = dialog.file() {
                    if let Some(path) = folder.path() {
                        match storage::load_project(&path) {
                            Ok(mut project) => {
                                let recovered = engine::recover_interrupted(&mut project);
                                if recovered {
                                    let _ = storage::save_project(&path, &project);
                                }
                                load_into(&ctx2, path, project);
                            }
                            Err(err) => show_error(&ctx2, &format!("cannot open: {err:#}")),
                        }
                    }
                }
            }
            dialog.close();
        });
        dialog.present();
    });
}

fn load_into(ctx: &Rc<UiCtx>, dir: PathBuf, project: Project) {
    *ctx.state.project_dir.borrow_mut() = Some(dir);
    *ctx.state.project.borrow_mut() = Some(project);
    *ctx.state.busy.borrow_mut() = false;
    ctx.add_frame.set_sensitive(true);
    refresh_all(ctx);
}

fn refresh_all(ctx: &Rc<UiCtx>) {
    let borrowed = ctx.state.project.borrow();
    let Some(project) = borrowed.as_ref() else {
        ctx.info_bar
            .set_text("No project open — create or open one to register media fragments.");
        return;
    };
    let dir = ctx
        .state
        .project_dir
        .borrow()
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    ctx.info_bar.set_text(&format!(
        "Project `{}` — target {} Hz — {} fragment(s) — dir: {dir}",
        project.project_id,
        project.target_rate,
        project.fragments.len()
    ));

    while let Some(child) = ctx.list_box.first_child() {
        ctx.list_box.remove(&child);
    }
    for fragment in &project.fragments {
        ctx.list_box.append(&build_fragment_row(fragment, ctx));
    }
    let busy = *ctx.state.busy.borrow();
    ctx.export_btn
        .set_sensitive(!busy && !project.fragments.is_empty());
    render_detail(ctx, None);
}

fn build_fragment_row(fragment: &Fragment, ctx: &Rc<UiCtx>) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    row.set_widget_name(&fragment.id);
    let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
    container.set_margin_top(4);
    container.set_margin_bottom(4);
    container.set_margin_start(6);
    container.set_margin_end(6);

    let title = gtk4::Label::new(Some(&format!(
        "{}  [{}]  {}  —  {} → {} Hz  gap {} ns",
        fragment.id,
        fragment.status.as_str(),
        fragment.device_label,
        fragment.source_rate,
        fragment.target_rate,
        fragment.gap_duration_ns,
    )));
    title.set_halign(gtk4::Align::Start);
    title.set_hexpand(true);
    title.set_xalign(0.0);
    container.append(&title);

    let run_label = if fragment.attempts == 0 {
        "Run"
    } else {
        "Rerun"
    };
    let run_btn = gtk4::Button::with_label(run_label);
    run_btn.add_css_class("pill");
    if fragment.status == FragmentStatus::Running || *ctx.state.busy.borrow() {
        run_btn.set_sensitive(false);
    }
    let id = fragment.id.clone();
    let ctx2 = ctx.clone();
    run_btn.connect_clicked(move |_| start_worker(&ctx2, id.clone()));
    container.append(&run_btn);

    row.set_child(Some(&container));
    row
}

fn render_detail(ctx: &Rc<UiCtx>, selected_id: Option<&str>) {
    let buffer = ctx.detail.buffer();
    let borrowed = ctx.state.project.borrow();
    let text = match (borrowed.as_ref(), selected_id) {
        (Some(project), Some(id)) => project
            .fragments
            .iter()
            .find(|f| f.id == id)
            .map(detail_text)
            .unwrap_or_else(|| "select a fragment".into()),
        (Some(project), None) => project
            .fragments
            .first()
            .map(detail_text)
            .unwrap_or_else(|| "no fragments yet".into()),
        (None, _) => "no project open".into(),
    };
    buffer.set_text(&text);
}

fn detail_text(fragment: &Fragment) -> String {
    let mut text = format!(
        "Fragment:        {}\nDevice:          {}\nSource:          {}\n\
         Source rate:     {} Hz\nTarget rate:     {} Hz\nStatus:          {}\n\
         Attempts:        {}\nGap start:       {} ns\nGap duration:    {} ns\n\
         Gap reason:      {}\nUpdated:         {}\n",
        fragment.id,
        fragment.device_label,
        fragment.source_path,
        fragment.source_rate,
        fragment.target_rate,
        fragment.status.as_str(),
        fragment.attempts,
        fragment.gap_start_ns,
        fragment.gap_duration_ns,
        fragment.gap_reason,
        fragment.updated_at,
    );
    if let Some(err) = &fragment.last_error {
        text.push_str(&format!("\nLast error:\n{err}\n"));
    }
    for evidence in &fragment.evidence {
        text.push_str("\n── Evidence (attempt ");
        text.push_str(&evidence.attempt.to_string());
        text.push_str(") ──\n");
        text.push_str(&flatten_evidence(evidence));
    }
    text
}

fn flatten_evidence(evidence: &crate::model::RunEvidence) -> String {
    format!(
        "finished_at:       {}\nGStreamer:         {}\ninput_sha256:      {}\n\
         input frames:      {}\noutput:            {}\noutput_sha256:     {}\n\
         output frames:     {} (expected {})\ngap frames:        {}\n\
         output bytes:      {}\npipeline:\n{}\n",
        evidence.finished_at,
        evidence.gst_version,
        evidence.input_sha256,
        evidence.input_frames,
        evidence.output_path,
        evidence.output_sha256,
        evidence.output_frames,
        evidence.expected_frames,
        evidence.gap_frames_inserted,
        evidence.output_bytes,
        evidence.pipeline_desc,
    )
}

fn wire_selection(list_box: &gtk4::ListBox, ctx: &Rc<UiCtx>) {
    let ctx = ctx.clone();
    list_box.connect_row_selected(move |_, row| {
        let id = row.map(|r| r.widget_name().to_string());
        render_detail(&ctx, id.as_deref());
    });
}

fn show_error(ctx: &Rc<UiCtx>, message: &str) {
    ctx.status_label.set_text(message);
    let dialog = gtk4::MessageDialog::new(
        Some(&ctx.window),
        gtk4::DialogFlags::MODAL,
        gtk4::MessageType::Error,
        gtk4::ButtonsType::Close,
        message,
    );
    dialog.connect_response(|dialog, _| dialog.close());
    dialog.present();
}

fn wire_add_form(form: &AddForm, ctx: &Rc<UiCtx>) {
    let browse_ctx = ctx.clone();
    let source_entry = form.source.clone();
    form.browse.connect_clicked(move |_| {
        let dialog = gtk4::FileChooserDialog::new(
            Some("Choose source media"),
            Some(&browse_ctx.window),
            gtk4::FileChooserAction::Open,
            &[
                ("Cancel", gtk4::ResponseType::Cancel),
                ("Select", gtk4::ResponseType::Accept),
            ],
        );
        let entry = source_entry.clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk4::ResponseType::Accept {
                if let Some(file) = dialog.file() {
                    if let Some(path) = file.path() {
                        entry.set_text(&path.display().to_string());
                    }
                }
            }
            dialog.close();
        });
        dialog.present();
    });

    let submit_ctx = ctx.clone();
    let form_clone = clone_form(form);
    form.submit.connect_clicked(move |_| {
        submit_add_form(&submit_ctx, &form_clone);
    });
}

struct FormSnapshot {
    device: String,
    source: String,
    source_rate: String,
    gap_start: String,
    gap_duration: String,
    gap_reason: String,
}

fn clone_form(form: &AddForm) -> FormSnapshot {
    FormSnapshot {
        device: form.device.text().to_string(),
        source: form.source.text().to_string(),
        source_rate: form.source_rate.text().to_string(),
        gap_start: form.gap_start.text().to_string(),
        gap_duration: form.gap_duration.text().to_string(),
        gap_reason: form.gap_reason.text().to_string(),
    }
}

fn submit_add_form(ctx: &Rc<UiCtx>, snapshot: &FormSnapshot) {
    let mut project_guard = ctx.state.project.borrow_mut();
    let Some(project) = project_guard.as_mut() else {
        show_error(ctx, "open or create a project first");
        return;
    };
    let parsed = parse_snapshot(snapshot, project.target_rate);
    let values = match parsed {
        Ok(values) => values,
        Err(err) => {
            show_error(ctx, &err);
            return;
        }
    };
    let id = project.next_fragment_id();
    let fragment = Fragment::new(
        id,
        values.device,
        values.source,
        values.source_rate,
        project.target_rate,
        values.gap_start,
        values.gap_duration,
        values.gap_reason,
        now_rfc3339(),
    );
    project.fragments.push(fragment);
    project.updated_at = now_rfc3339();
    let dir = ctx.state.project_dir.borrow().clone();
    if let Some(dir) = dir {
        if let Err(err) = storage::save_project(&dir, project) {
            show_error(ctx, &format!("cannot save project: {err:#}"));
            return;
        }
    }
    drop(project_guard);
    refresh_all(ctx);
}

struct ParsedFragment {
    device: String,
    source: String,
    source_rate: u32,
    gap_start: u64,
    gap_duration: u64,
    gap_reason: String,
}

fn parse_snapshot(snapshot: &FormSnapshot, target_rate: u32) -> Result<ParsedFragment, String> {
    if snapshot.device.trim().is_empty() {
        return Err("device label is required".into());
    }
    if snapshot.source.trim().is_empty() {
        return Err("source file path is required".into());
    }
    let source_rate: u32 = snapshot
        .source_rate
        .parse()
        .map_err(|_| "source rate must be an integer in Hz".to_string())?;
    if source_rate == 0 {
        return Err("source rate must be greater than zero".into());
    }
    let gap_start: u64 = snapshot
        .gap_start
        .parse()
        .map_err(|_| "gap start must be nanoseconds as integer".to_string())?;
    let gap_duration: u64 = snapshot
        .gap_duration
        .parse()
        .map_err(|_| "gap duration must be nanoseconds as integer".to_string())?;
    if target_rate == 0 {
        return Err("project target rate is zero".into());
    }
    Ok(ParsedFragment {
        device: snapshot.device.trim().to_string(),
        source: snapshot.source.trim().to_string(),
        source_rate,
        gap_start,
        gap_duration,
        gap_reason: snapshot.gap_reason.trim().to_string(),
    })
}

fn wire_export_button(btn: &gtk4::Button, ctx: &Rc<UiCtx>) {
    let ctx = ctx.clone();
    btn.connect_clicked(move |_| {
        let Some(dir) = ctx.state.project_dir.borrow().clone() else {
            return;
        };
        let mut guard = ctx.state.project.borrow_mut();
        let Some(project) = guard.as_mut() else {
            return;
        };
        match review::export_bundle(project, &dir) {
            Ok(report) => {
                ctx.status_label.set_text(&format!(
                    "exported review bundle: {}",
                    report.bundle_dir.display()
                ));
            }
            Err(err) => show_error(&ctx, &format!("export refused: {err:#}")),
        }
    });
}

/// Spawn the GStreamer worker. The project file is the synchronization
/// boundary: this thread owns its own in-memory copy and writes every
/// transition itself, while the GUI reloads state on completion.
fn start_worker(ctx: &Rc<UiCtx>, id: String) {
    if *ctx.state.busy.borrow() {
        return;
    }
    let Some(dir) = ctx.state.project_dir.borrow().clone() else {
        return;
    };
    *ctx.state.busy.borrow_mut() = true;
    refresh_all(ctx);
    ctx.status_label
        .set_text(&format!("processing {id} — you may close the window; progress is saved"));

    let (tx, rx) = mpsc::channel::<WorkerMsg>();
    let worker_id = id.clone();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(|| worker_run(&dir, &worker_id))
            .unwrap_or_else(|_| Err("worker thread panicked".into()));
        let _ = tx.send(WorkerMsg::Finished {
            id: worker_id,
            result,
        });
    });

    let ctx2 = ctx.clone();
    attach_worker_poll(ctx2, rx);
}

/// Poll the worker channel on the GTK main loop. `try_recv` is non-blocking,
/// so the UI stays responsive and the timer removes itself on completion.
fn attach_worker_poll(ctx: Rc<UiCtx>, rx: Receiver<WorkerMsg>) {
    glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
        match rx.try_recv() {
            Ok(WorkerMsg::Finished { id, result }) => {
                on_worker_finished(&ctx, &id, result);
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

fn worker_run(
    dir: &std::path::Path,
    id: &str,
) -> Result<crate::model::RunEvidence, String> {
    let mut project = storage::load_project(dir).map_err(|e| format!("{e:#}"))?;
    engine::recover_interrupted(&mut project);
    engine::run_fragment(&mut project, dir, id).map_err(|e| format!("{e:#}"))
}

fn on_worker_finished(
    ctx: &Rc<UiCtx>,
    id: &str,
    result: Result<crate::model::RunEvidence, String>,
) {
    *ctx.state.busy.borrow_mut() = false;
    if let Some(dir) = ctx.state.project_dir.borrow().clone() {
        match storage::load_project(&dir) {
            Ok(project) => {
                *ctx.state.project.borrow_mut() = Some(project);
            }
            Err(err) => show_error(ctx, &format!("cannot reload project: {err:#}")),
        }
    }
    match result {
        Ok(evidence) => ctx.status_label.set_text(&format!(
            "{id} done — {} output frames, hash {}",
            evidence.output_frames, evidence.output_sha256
        )),
        Err(err) => {
            ctx.status_label
                .set_text(&format!("{id} failed — rerun when ready: {err}"));
        }
    }
    refresh_all(ctx);
}

