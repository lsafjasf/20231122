use gtk4::prelude::*;
use gtk4::{glib, Application, ApplicationWindow, Label};
use std::error::Error;
use std::sync::{Arc, Mutex};

const APP_ID: &str = "dev.llsm.replay.Monitor";

/// GTK4 low-latency monitor: shows live latency stats for the v1
/// control snapshot. Headless environments simply skip the window —
/// the experiment itself does not depend on a display.
pub fn run() -> Result<(), Box<dyn Error>> {
    let has_display =
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    if !has_display {
        eprintln!("monitor: no display available, skipping GTK4 window (headless)");
        return Ok(());
    }
    if gtk4::init().is_err() {
        eprintln!("monitor: no display available, skipping GTK4 window (headless)");
        return Ok(());
    }
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run_with_args(&[] as &[&str]);
    Ok(())
}

fn build_ui(app: &Application) {
    let stats: Arc<Mutex<Option<crate::pipeline::LatencyStats>>> = Arc::new(Mutex::new(None));

    let label = Label::new(Some("starting low-latency pipeline…"));
    let window = ApplicationWindow::builder()
        .application(app)
        .title("Low-Latency Stream Monitor")
        .default_width(420)
        .default_height(120)
        .child(&label)
        .build();
    window.present();

    let shared = Arc::clone(&stats);
    std::thread::spawn(move || {
        if let Ok(snap) = crate::snapshot::load(std::path::Path::new("snapshots/v1.json")) {
            if let Ok(out) = crate::pipeline::run_monitored(&snap.pipeline, snap.duration_ms) {
                *shared.lock().unwrap() = Some(out.stats);
            }
        }
    });

    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        let text = match stats.lock().unwrap().clone() {
            Some(s) => format!(
                "buffers: {}   min: {:.2} ms   mean: {:.2} ms   max: {:.2} ms",
                s.buffers, s.min_ms, s.mean_ms, s.max_ms
            ),
            None => "collecting…".to_string(),
        };
        label.set_text(&text);
        glib::ControlFlow::Continue
    });
}
