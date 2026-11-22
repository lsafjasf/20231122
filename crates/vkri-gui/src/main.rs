//! GTK4 interface for vkri.
//!
//! The UI is a thin presentation layer over `vkri-core::ProjectService`.
//! Long-running extraction runs on a worker thread driven by the same
//! `CancellationToken` as the CLI; progress and completion hop back to the GTK
//! main loop via channels. Closing the window mid-run cancels cleanly, the
//! project is re-saved with an `interrupted` status, and reopening restores
//! every field, ordering and retained evidence.

mod dialogs;
mod state;
mod widgets;
mod workers;

use gtk::prelude::*;
use gtk::{gio, glib, Application};

use crate::state::AppState;

fn main() -> glib::ExitCode {
    vkri_core::media::init_gstreamer().expect("failed to initialize GStreamer");
    let application = Application::builder()
        .application_id("org.vkri.RebuildIndex")
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    application.connect_activate(|app| {
        let state = AppState::new();
        let window = widgets::build_main_window(app, state);
        window.present();
    });

    application.run()
}
