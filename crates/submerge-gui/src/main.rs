mod gst;
mod window;

use gtk4::prelude::*;

fn main() -> gtk4::glib::ExitCode {
    if let Err(e) = gst::init() {
        eprintln!("{}", e);
    }

    let app = gtk4::Application::builder()
        .application_id("dev.submerge.MergeTool")
        .build();

    app.connect_activate(|app| {
        let window = window::build_window(app);
        window.present();
    });

    app.run()
}
