//! 音频噪声频谱标注 · 来源检查页
//! 用法:
//!   noise-provenance          启动 GTK4 界面
//!   noise-provenance --demo   生成演示数据（两次中途取消 + 完成 + 失败）后退出

mod analysis;
mod demo;
mod model;
mod ui;

use gtk4::prelude::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--demo") {
        match demo::build_demo_journal() {
            Ok(journal) => {
                println!("演示日志已写入 {}", model::JOURNAL_PATH);
                for run in &journal.runs {
                    println!(
                        "  {} {} 进度 {:.0}% 结论 {} 条",
                        run.id,
                        run.status.label(),
                        run.progress * 100.0,
                        run.conclusions.len()
                    );
                }
            }
            Err(e) => {
                eprintln!("生成演示数据失败: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    gstreamer::init().expect("初始化 GStreamer 失败");
    // 注：不设置 application_id —— 本环境的 D-Bus 会话注册会导致应用立即退出。
    let app = gtk4::Application::builder().build();
    // --shot=路径[:运行序号] 截图模式（可选指定选中的运行记录）
    let shot = args.iter().find_map(|a| a.strip_prefix("--shot=")).map(|spec| {
        match spec.rsplit_once(':') {
            Some((p, i)) if i.chars().all(|c| c.is_ascii_digit()) && !i.is_empty() => {
                (p.to_string(), i.parse::<usize>().ok())
            }
            _ => (spec.to_string(), None),
        }
    });
    if let Some((_, Some(idx))) = &shot {
        ui::set_initial_selection(*idx);
    }
    app.connect_activate(move |app| {
        let window = ui::build_ui(app);
        if let Some((path, _)) = shot.clone() {
            let app = app.clone();
            gtk4::glib::timeout_add_local_once(
                std::time::Duration::from_millis(1500),
                move || {
                    save_screenshot(&window, &path);
                    app.quit();
                },
            );
        }
    });
    app.run_with_args::<&str>(&[]); // 自定义参数已自行解析，避免 GApplication 报错
}

/// 将窗口快照保存为 PNG（配合 GDK_BACKEND=headless 做界面自检）。
fn save_screenshot(window: &gtk4::ApplicationWindow, path: &str) {
    let snapshot = gtk4::Snapshot::new();
    if let Some(child) = window.child() {
        window.snapshot_child(&child, &snapshot);
    }
    let Some(node) = snapshot.to_node() else {
        eprintln!("截图失败：无法生成渲染节点");
        return;
    };
    let Some(surface) = window.surface() else {
        eprintln!("截图失败：窗口无 surface");
        return;
    };
    let Some(renderer) = gtk4::gsk::Renderer::for_surface(&surface) else {
        eprintln!("截图失败：无渲染器");
        return;
    };
    let texture = renderer.render_texture(&node, None);
    if texture.save_to_png(path).is_ok() {
        eprintln!("截图已保存: {path}");
    } else {
        eprintln!("截图保存失败: {path}");
    }
    renderer.unrealize();
}
