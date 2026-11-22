//! GTK4 application shell.
//!
//! The shell renders the replay journal: a header with the replay status, a
//! list of every recorded step (input -> state -> outcome), and the final
//! timeline. It is deliberately driven by the same pure `Engine`, so the GUI
//! shows exactly what headless replay computed. It renders headlessly under
//! the Broadway backend (`GDK_BACKEND=broadway`).

use crate::journal::JournalEntry;
use crate::model::{Command, Outcome};
use gtk::glib;
use gtk::gsk;
use gtk::prelude::*;
use gtk4 as gtk;

/// Build and run the GTK4 application. `snapshot_dir` receives a rendered
/// PNG snapshot when running under a headless backend.
pub fn run(
    app_id: &str,
    entries: &[JournalEntry],
    replay_ok: bool,
    snapshot_path: Option<&str>,
) -> glib::ExitCode {
    let application = gtk::Application::new(Some(app_id), gtk::gio::ApplicationFlags::default());
    let entries = entries.to_vec();
    let snapshot_path = snapshot_path.map(str::to_string);
    application.connect_activate(move |app| {
        build_window(app, &entries, replay_ok, snapshot_path.as_deref())
    });
    let empty: Vec<String> = Vec::new();
    application.run_with_args(&empty)
}

fn build_window(
    app: &gtk::Application,
    entries: &[JournalEntry],
    replay_ok: bool,
    snapshot_path: Option<&str>,
) {
    let window = gtk::ApplicationWindow::new(app);
    window.set_title(Some("VFR Replay Lab"));
    window.set_default_size(760, 520);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);

    let header = gtk::Label::new(Some(&format!(
        "VFR clip preview — replay {} ({} steps)",
        if replay_ok { "OK" } else { "FAILED" },
        entries.len()
    )));
    header.add_css_class("title-2");
    header.add_css_class(if replay_ok {
        "success-heading"
    } else {
        "error-heading"
    });
    root.append(&header);

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    for entry in entries {
        let row = gtk::Label::new(Some(&format_row(entry)));
        row.set_xalign(0.0);
        row.set_margin_top(4);
        row.set_margin_bottom(4);
        list.append(&row);
    }

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_vexpand(true);
    scroller.set_child(Some(&list));
    root.append(&scroller);

    let last = entries.last();
    let footer = gtk::Label::new(Some(&format!(
        "Final timeline: {:?} | revision: {}",
        last.map(|e| e.state_after.timeline.clone())
            .unwrap_or_default(),
        last.map(|e| e.state_after.revision).unwrap_or(0)
    )));
    footer.set_xalign(0.0);
    root.append(&footer);

    window.set_child(Some(&root));
    window.present();

    if let Some(path) = snapshot_path {
        let root_widget: gtk::Widget = root.upcast();
        let target = path.to_string();
        let window = window.clone();
        // Wait for the first frame to be laid out before snapshotting, then
        // quit so headless/CI runs terminate after capturing evidence.
        gtk::glib::idle_add_local_once(move || {
            snapshot(&root_widget, &target);
            window.close();
        });
    }
}

fn format_row(entry: &JournalEntry) -> String {
    let input = match &entry.command {
        Command::AddClip { clip } => format!("AddClip({} frames={})", clip.id, clip.pts_ns.len()),
        Command::Place { clip_id, position } => format!("Place({clip_id} @ {position})"),
        Command::Remove { clip_id } => format!("Remove({clip_id})"),
        Command::SetRate { fps_num, fps_den } => format!("SetRate({fps_num}/{fps_den})"),
        Command::Swap {
            position_a,
            position_b,
        } => format!("Swap({position_a}<->{position_b})"),
        Command::UnknownExtension { name, .. } => format!("UnknownExtension({name})"),
    };
    let result = match &entry.outcome {
        Outcome::Accepted { detail } => format!("accepted: {detail}"),
        Outcome::Rejected { reason } => format!("rejected: {reason}"),
        Outcome::Duplicate { original_seq } => format!("duplicate of seq {original_seq}"),
        Outcome::UnknownIgnored { name } => format!("unknown extension ignored: {name}"),
    };
    format!(
        "#{:02} [{}] {} -> {} | rev={}",
        entry.seq, entry.idem_key, input, result, entry.state_after.revision
    )
}

fn snapshot(widget: &gtk::Widget, path: &str) {
    let width = widget.width().max(1);
    let height = widget.height().max(1);

    let renderer = gsk::CairoRenderer::new();
    if renderer.realize(None).is_err() {
        eprintln!("snapshot: renderer realize failed");
        return;
    }

    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let viewport = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    let snap = gtk::Snapshot::new();
    paintable.snapshot(&snap, width as f64, height as f64);
    let node = match snap.to_node() {
        Some(node) => node,
        None => {
            eprintln!("snapshot: no render node ({width}x{height})");
            renderer.unrealize();
            return;
        }
    };
    let texture = renderer.render_texture(&node, Some(&viewport));
    renderer.unrealize();

    match texture.save_to_png(path) {
        Ok(_) => println!("snapshot saved: {path} ({width}x{height})"),
        Err(e) => eprintln!("snapshot failed: {e}"),
    }
}
