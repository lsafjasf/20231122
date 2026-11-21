//! GTK4 + GStreamer front-end for plc-eval.
//!
//! Load a clean/degraded/manifest triple, inspect every gap (duration,
//! chosen strategy, waveform energy difference, audible-artifact flag),
//! and listen to the original, degraded and recovered clips. Playback
//! goes through a GStreamer `playbin`; recovery is computed by the
//! shared `plc-eval` core and written to a temp WAV for playback.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::glib;
use gtk4::prelude::*;

use gstreamer as gst;
use gstreamer::prelude::*;

use plc_eval::eval;
use plc_eval::loss::LossManifest;
use plc_eval::wav::Wav;
use plc_eval::Strategy;

struct Session {
    clean: Wav,
    degraded: Wav,
    manifest: LossManifest,
    reports: Vec<eval::GapReport>,
}

struct Ui {
    clean_entry: gtk4::Entry,
    degraded_entry: gtk4::Entry,
    manifest_entry: gtk4::Entry,
    gap_list: gtk4::ListBox,
    summary: gtk4::Label,
    status: gtk4::Label,
    strategy: gtk4::DropDown,
}

fn main() -> glib::ExitCode {
    gst::init().expect("failed to initialise GStreamer");
    let app = gtk4::Application::new(Some("dev.plc.Eval"), Default::default());
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &gtk4::Application) {
    let window = gtk4::ApplicationWindow::new(app);
    window.set_title(Some("PLC 离线掩蔽评估"));
    window.set_default_size(860, 640);

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    root.set_margin_top(8);
    root.set_margin_bottom(8);
    root.set_margin_start(8);
    root.set_margin_end(8);

    // --- file selectors ---
    let grid = gtk4::Grid::new();
    grid.set_column_spacing(6);
    grid.set_row_spacing(6);
    let clean_entry = gtk4::Entry::new();
    clean_entry.set_text("samples/clean.wav");
    clean_entry.set_hexpand(true);
    let degraded_entry = gtk4::Entry::new();
    degraded_entry.set_text("samples/burst.wav");
    degraded_entry.set_hexpand(true);
    let manifest_entry = gtk4::Entry::new();
    manifest_entry.set_text("samples/burst.manifest.txt");
    manifest_entry.set_hexpand(true);
    grid.attach(&gtk4::Label::new(Some("原始 (clean)")), 0, 0, 1, 1);
    grid.attach(&clean_entry, 1, 0, 1, 1);
    grid.attach(&gtk4::Label::new(Some("降级 (丢包)")), 0, 1, 1, 1);
    grid.attach(&degraded_entry, 1, 1, 1, 1);
    grid.attach(&gtk4::Label::new(Some("丢包清单")), 0, 2, 1, 1);
    grid.attach(&manifest_entry, 1, 2, 1, 1);
    root.append(&grid);

    // --- controls ---
    let controls = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let load_btn = gtk4::Button::with_label("加载并评估");
    let play_clean = gtk4::Button::with_label("试听原始");
    let play_degraded = gtk4::Button::with_label("试听降级");
    let play_recovered = gtk4::Button::with_label("试听恢复");
    let strategy = gtk4::DropDown::from_strings(&["best", "silence", "repeat", "interpolate"]);
    controls.append(&load_btn);
    controls.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));
    controls.append(&play_clean);
    controls.append(&play_degraded);
    controls.append(&gtk4::Label::new(Some("恢复策略:")));
    controls.append(&strategy);
    controls.append(&play_recovered);
    root.append(&controls);

    // --- gap table ---
    let header = gtk4::Label::new(Some(
        "  #      起点(ms)   时长(ms)  声道    策略         能量差(dB)  突变    伪影",
    ));
    header.set_xalign(0.0);
    header.add_css_class("monospace");
    root.append(&header);
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_vexpand(true);
    let gap_list = gtk4::ListBox::new();
    gap_list.add_css_class("monospace");
    scroll.set_child(Some(&gap_list));
    root.append(&scroll);

    let summary = gtk4::Label::new(None);
    summary.set_xalign(0.0);
    summary.add_css_class("monospace");
    root.append(&summary);
    let status = gtk4::Label::new(Some("未加载"));
    status.set_xalign(0.0);
    root.append(&status);

    let ui = Rc::new(Ui {
        clean_entry,
        degraded_entry,
        manifest_entry,
        gap_list,
        summary,
        status,
        strategy,
    });
    let session: Rc<RefCell<Option<Session>>> = Rc::new(RefCell::new(None));
    let player: Rc<RefCell<Option<gst::Element>>> = Rc::new(RefCell::new(None));

    load_btn.connect_clicked({
        let ui = ui.clone();
        let session = session.clone();
        move |_| match load_session(&ui) {
            Ok(s) => {
                ui.status.set_text(&format!(
                    "已加载: {} 个缺口, 清单类型 {}",
                    s.reports.len(),
                    s.manifest.kind
                ));
                *session.borrow_mut() = Some(s);
            }
            Err(e) => ui.status.set_text(&format!("加载失败: {e:#}")),
        }
    });

    play_clean.connect_clicked({
        let ui = ui.clone();
        let player = player.clone();
        move |_| {
            play_file(
                &player,
                &ui.status,
                PathBuf::from(ui.clean_entry.text().as_str()),
            )
        }
    });
    play_degraded.connect_clicked({
        let ui = ui.clone();
        let player = player.clone();
        move |_| {
            play_file(
                &player,
                &ui.status,
                PathBuf::from(ui.degraded_entry.text().as_str()),
            )
        }
    });
    play_recovered.connect_clicked({
        let ui = ui.clone();
        let session = session.clone();
        let player = player.clone();
        move |_| {
            let Some(s) = session.borrow().as_ref() else {
                ui.status.set_text("请先加载并评估");
                return;
            };
            let selected = ui.strategy.selected();
            let wav = match selected {
                1 => eval::recover_with(&s.degraded, &s.manifest, Strategy::Silence),
                2 => eval::recover_with(&s.degraded, &s.manifest, Strategy::RepeatLastFrame),
                3 => eval::recover_with(&s.degraded, &s.manifest, Strategy::Interpolate),
                _ => eval::recover(&s.degraded, &s.manifest, &s.reports),
            };
            let path = std::env::temp_dir().join("plc_eval_recovered.wav");
            match wav.write(&path) {
                Ok(()) => play_file(&player, &ui.status, path),
                Err(e) => ui.status.set_text(&format!("写出失败: {e:#}")),
            }
        }
    });

    window.set_child(Some(&root));
    window.present();
}

fn load_session(ui: &Ui) -> anyhow::Result<Session> {
    let clean = Wav::read(&PathBuf::from(ui.clean_entry.text().as_str()))?;
    let degraded = Wav::read(&PathBuf::from(ui.degraded_entry.text().as_str()))?;
    let text = std::fs::read_to_string(ui.manifest_entry.text().as_str())?;
    let manifest = LossManifest::from_text(&text)?;
    let reports = eval::evaluate(&clean, &degraded, &manifest);

    // refill the gap list
    while let Some(row) = ui.gap_list.first_child() {
        ui.gap_list.remove(&row);
    }
    for r in &reports {
        let m = r.best_metrics();
        let line = format!(
            "{:>3}   {:>8}   {:>7}   {:>3}   {:<12} {:>+8.2}   {:>5.2}   {}",
            r.gap_index,
            r.start_ms,
            r.duration_ms,
            r.channel,
            r.best.label(),
            m.energy_diff_db,
            m.edge_discontinuity,
            if m.artifact { "YES" } else { "no" },
        );
        let label = gtk4::Label::new(Some(&line));
        label.set_xalign(0.0);
        if m.artifact {
            label.add_css_class("error");
        }
        ui.gap_list.append(&label);
    }
    ui.summary.set_text(&eval::summarize(&reports));
    Ok(Session {
        clean,
        degraded,
        manifest,
        reports,
    })
}

fn play_file(player: &Rc<RefCell<Option<gst::Element>>>, status: &gtk4::Label, path: PathBuf) {
    // stop any current playback
    if let Some(p) = player.borrow_mut().take() {
        let _ = p.set_state(gst::State::Null);
    }
    let canon = match path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            status.set_text(&format!("文件不存在: {} ({e})", path.display()));
            return;
        }
    };
    let uri = match glib::filename_to_uri(&canon, None) {
        Ok(u) => u,
        Err(e) => {
            status.set_text(&format!("URI 转换失败: {e}"));
            return;
        }
    };
    let playbin = match gst::ElementFactory::make("playbin").build() {
        Ok(p) => p,
        Err(e) => {
            status.set_text(&format!("无法创建 playbin: {e}"));
            return;
        }
    };
    playbin.set_property("uri", uri.as_str());
    if let Err(e) = playbin.set_state(gst::State::Playing) {
        status.set_text(&format!("播放失败: {e}"));
        return;
    }
    status.set_text(&format!("播放中: {}", canon.display()));
    *player.borrow_mut() = Some(playbin);
}
