mod ui;

use subtitle_safe_review::analysis;

fn main() -> gtk4::glib::ExitCode {
    analysis::ensure_gst_init();
    ui::run()
}
