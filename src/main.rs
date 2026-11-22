mod app;
mod audio;
mod demo;
mod model;
mod ui;

fn main() {
    use gtk4::prelude::*;

    if let Err(error) = audio::init() {
        eprintln!("启动失败: {error:#}");
        std::process::exit(1);
    }
    let application = gtk4::Application::builder()
        .application_id("com.peakguard.desktop")
        .build();
    application.connect_activate(|application| {
        if let Err(error) = app::activate(application) {
            eprintln!("界面启动失败: {error:#}");
        }
    });
    application.run();
}
