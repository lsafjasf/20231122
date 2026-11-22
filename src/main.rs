mod gst_probe;
mod model;
mod ui;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;

use crate::model::Session;
use crate::ui::shell::build_window;

fn main() -> gtk::glib::ExitCode {
    gst::init().expect("GStreamer 初始化失败");
    let screenshot_path = screenshot_argument();

    let app = gtk::Application::builder()
        .application_id("io.relay.backpressure-inspector")
        .build();

    app.connect_startup(|_| load_css());
    app.connect_activate(move |app| {
        let session = match Session::load() {
            Ok(session) => session,
            Err(error) => {
                eprintln!("{error}");
                return;
            }
        };
        let window = build_window(app, &session);
        window.present();
        if let Some(path) = screenshot_path.take() {
            let window_clone = window.clone();
            let app_clone = app.clone();
            gtk::glib::timeout_add_local_once(Duration::from_millis(1_500), move || {
                let window_ref: &gtk::Window = window_clone.upcast_ref();
                match capture_window(window_ref, &path) {
                    Ok(()) => println!("screenshot: {path}"),
                    Err(error) => eprintln!("screenshot failed: {error}"),
                }
                app_clone.quit();
            });
        }
    });

    app.run_with_args(&["backpressure-inspector"])
}

fn screenshot_argument() -> Rc<RefCell<Option<String>>> {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .iter()
        .position(|arg| arg == "--screenshot")
        .and_then(|index| args.get(index + 1).cloned())
        .or_else(|| {
            args.iter()
                .find_map(|arg| arg.strip_prefix("--screenshot=").map(str::to_string))
        });
    Rc::new(RefCell::new(path))
}

fn capture_window(window: &gtk::Window, path: &str) -> Result<(), String> {
    let width = f64::from(window.width().max(1));
    let height = f64::from(window.height().max(1));
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width, height);
    let node = snapshot
        .to_node()
        .ok_or_else(|| "窗口没有可渲染的节点".to_string())?;
    let renderer = gtk::gsk::CairoRenderer::new();
    renderer
        .realize(None)
        .map_err(|error| format!("Cairo renderer 初始化失败: {error}"))?;
    let texture = renderer.render_texture(&node, None);
    texture
        .save_to_png(path)
        .map_err(|error| format!("写入 PNG 失败: {error}"))
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("../assets/style.css"));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
