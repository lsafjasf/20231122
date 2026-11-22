mod analyzer;
mod fixture;
mod gst_probe;
mod model;
mod ui;

fn main() -> gtk4::glib::ExitCode {
    ui::run()
}
