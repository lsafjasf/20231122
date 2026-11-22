mod app;
mod detail;

use gtk4::prelude::*;

fn main() -> gtk4::glib::ExitCode {
    let app = gtk4::Application::builder()
        .application_id("dev.rotfix.provenance")
        .build();
    app.connect_activate(app::build);
    app.run()
}
