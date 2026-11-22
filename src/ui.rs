//! GTK4 复核界面：波形、首错定位、条目列表、证据帧、一键重放。
use crate::record::{self, Record};
use crate::replay::{self, ReplayReport};
use crate::ui_support::load_png_texture;
use gtk4::prelude::*;
use gtk4::{gdk, glib};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
struct State {
    record_path: Option<PathBuf>,
    record: Option<Record>,
    report: Option<ReplayReport>,
}
pub fn run(record: Option<PathBuf>) -> anyhow::Result<()> {
    let app = gtk4::Application::builder()
        .application_id("dev.safeframe.audit")
        .build();
    app.connect_activate(move |app| build_ui(app, record.clone()));
    app.run_with_args(&[] as &[&str]);
    Ok(())
}
fn build_ui(app: &gtk4::Application, initial: Option<PathBuf>) {
    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .title("画面安全框检测 · 复核")
        .default_width(1100)
        .default_height(760)
        .build();
    let state = Rc::new(RefCell::new(State {
        record_path: None,
        record: None,
        report: None,
    }));
    let header = gtk4::HeaderBar::new();
    let open_btn = gtk4::Button::with_label("打开记录");
    let replay_btn = gtk4::Button::with_label("重放复核");
    replay_btn.add_css_class("suggested-action");
    header.pack_start(&open_btn);
    header.pack_end(&replay_btn);
    window.set_titlebar(Some(&header));
    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    root.set_margin_top(6);
    root.set_margin_bottom(6);
    root.set_margin_start(6);
    root.set_margin_end(6);
    let info = gtk4::Label::new(Some("未加载记录"));
    info.set_xalign(0.0);
    info.add_css_class("title-4");
    root.append(&info);
    let wave_area = gtk4::DrawingArea::new();
    wave_area.set_content_height(220);
    wave_area.set_hexpand(true);
    root.append(&wave_area);
    let paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    paned.set_vexpand(true);
    paned.set_position(560);
    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::None);
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_child(Some(&list));
    scroll.set_min_content_width(540);
    paned.set_start_child(Some(&scroll));
    let right = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    let evidence = gtk4::Picture::new();
    evidence.set_content_fit(gtk4::ContentFit::Contain);
    evidence.set_vexpand(true);
    let verdict = gtk4::Label::new(None);
    verdict.set_wrap(true);
    verdict.set_xalign(0.0);
    right.append(&evidence);
    right.append(&verdict);
    paned.set_end_child(Some(&right));
    root.append(&paned);
    window.set_child(Some(&root));
    {
        let state = state.clone();
        wave_area.set_draw_func(move |area, cr, w, h| {
            let st = state.borrow();
            let Some(rec) = &st.record else { return };
            let wave = rec.waveform();
            let fe = rec.entries.iter().position(|e| e.violation);
            let threshold = rec
                .header
                .as_ref()
                .map(|hd| hd.params.max_ratio)
                .unwrap_or_else(|| crate::params::SafeFrameParams::default().max_ratio);
            let (w, h) = (w as f64, h as f64);
            cr.set_source_rgb(0.98, 0.98, 0.98);
            let _ = cr.paint();
            if wave.is_empty() {
                return;
            }
            let max_v = wave
                .iter()
                .cloned()
                .fold(threshold * 1.5, f64::max)
                .max(1e-9);
            let y_of = |v: f64| h - 10.0 - (v / max_v) * (h - 20.0);
            // 阈值线（红）
            cr.set_source_rgb(0.9, 0.24, 0.24);
            cr.set_line_width(1.0);
            cr.move_to(0.0, y_of(threshold));
            cr.line_to(w, y_of(threshold));
            let _ = cr.stroke();
            // 波形（蓝）
            cr.set_source_rgb(0.16, 0.35, 0.86);
            cr.set_line_width(1.6);
            let n = wave.len();
            for i in 0..n {
                let x = i as f64 * w / (n - 1).max(1) as f64;
                let y = y_of(wave[i]);
                if i == 0 {
                    cr.move_to(x, y);
                } else {
                    cr.line_to(x, y);
                }
            }
            let _ = cr.stroke();
            // 第一个错误状态（竖红标记）
            if let Some(fe) = fe {
                let x = fe as f64 * w / (n - 1).max(1) as f64;
                cr.set_source_rgb(1.0, 0.0, 0.0);
                cr.set_line_width(2.0);
                cr.move_to(x, 0.0);
                cr.line_to(x, h);
                let _ = cr.stroke();
            }
            let _ = area;
        });
    }
    // 加载记录 → 刷新信息栏 / 列表 / 波形 / 证据
    let refresh: Rc<dyn Fn()> = {
        let (state, info, list, verdict, evidence, wave_area) = (
            state.clone(),
            info.clone(),
            list.clone(),
            verdict.clone(),
            evidence.clone(),
            wave_area.clone(),
        );
        Rc::new(move || {
            let st = state.borrow();
            let Some(rec) = &st.record else { return };
            let head = match (&rec.header, rec.format) {
                (Some(h), _) => format!(
                    "格式 v2 · 媒体 {} · params_hash={} · 阈值 {}",
                    h.media, h.params_hash, h.params.max_ratio
                ),
                (None, record::Format::V1) => "格式 v1（旧）· 无参数头，重放用默认参数".to_string(),
                _ => String::new(),
            };
            let recov = match &rec.recovered {
                Some(m) => format!(" · 已恢复(丢弃 {} 字节)", m.dropped_bytes),
                None if rec.tail_garbage_bytes > 0 => {
                    format!(" · 含 {} 字节垃圾尾(未恢复!)", rec.tail_garbage_bytes)
                }
                None => String::new(),
            };
            info.set_text(&format!(
                "{head}{recov} · {} 条记录 · 波形 CRC: {}",
                rec.entries.len(),
                match rec.wave_crc_ok {
                    Some(true) => "通过",
                    Some(false) => "失败!",
                    None => "无 eof(截断)",
                }
            ));
            while let Some(row) = list.first_child() {
                list.remove(&row);
            }
            let first_err = rec.entries.iter().position(|e| e.violation);
            for (i, e) in rec.entries.iter().enumerate() {
                let mark = if Some(i) == first_err {
                    " ◀ 第一个错误状态"
                } else {
                    ""
                };
                let row = gtk4::Label::new(Some(&format!(
                    "seq={:<4} frame={:<4} pts={:>6}ms track={} metric={:.6} {}{}",
                    e.seq,
                    e.frame,
                    e.pts_ms,
                    e.track,
                    e.metric,
                    if e.violation { "违规" } else { "正常" },
                    mark
                )));
                row.set_xalign(0.0);
                row.add_css_class("monospace");
                if Some(i) == first_err {
                    row.add_css_class("error");
                }
                list.append(&row);
            }
            // 证据帧：记录同级的 evidence/ 目录
            if let Some(rp) = &st.record_path {
                let evdir = rp.parent().map(|p| p.join("evidence"));
                if let Some(evdir) = evdir {
                    if let Ok(rd) = std::fs::read_dir(&evdir) {
                        let mut pngs: Vec<_> = rd
                            .flatten()
                            .map(|d| d.path())
                            .filter(|p| {
                                p.file_name()
                                    .map(|n| n.to_string_lossy().starts_with("first_error_frame_"))
                                    .unwrap_or(false)
                            })
                            .collect();
                        pngs.sort();
                        if let Some(p) = pngs.first() {
                            match load_png_texture(p) {
                                Ok(t) => evidence.set_paintable(Some(&t)),
                                Err(e) => verdict.set_text(&format!("证据解码失败: {e}")),
                            }
                        }
                    }
                }
            }
            wave_area.queue_draw();
            let _ = &verdict;
        })
    };
    // 打开记录
    {
        let (state, refresh, window) = (state.clone(), refresh.clone(), window.clone());
        open_btn.connect_clicked(move |_| {
            let dlg = gtk4::FileDialog::builder().title("选择检测记录").build();
            let (state, refresh) = (state.clone(), refresh.clone());
            dlg.open(Some(&window), gtk4::gio::Cancellable::NONE, move |res| {
                if let Ok(f) = res {
                    if let Some(p) = f.path() {
                        match record::load(&p) {
                            Ok(rec) => {
                                let mut st = state.borrow_mut();
                                st.record_path = Some(PathBuf::from(p));
                                st.record = Some(rec);
                                st.report = None;
                                drop(st);
                                refresh();
                            }
                            Err(e) => eprintln!("加载失败: {e}"),
                        }
                    }
                }
            });
        });
    }
    // 重放复核：后台线程跑 GStreamer，主线程轮询结果
    {
        let state = state.clone();
        let (tx, rx) = std::sync::mpsc::channel::<ReplayReport>();
        let verdict_c = verdict.clone();
        let state_c = state.clone();
        replay_btn.connect_clicked(move |_| {
            let rec_path = state_c.borrow().record_path.clone();
            let Some(rec_path) = rec_path else { return };
            verdict_c.set_text("重放中…");
            let tx = tx.clone();
            std::thread::spawn(move || {
                if let Ok(rep) = replay::replay(&rec_path, None) {
                    let _ = tx.send(rep);
                }
            });
        });
        let (state, refresh) = (state.clone(), refresh.clone());
        glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
            while let Ok(rep) = rx.try_recv() {
                let first_mm = rep.first_mismatch.clone();
                let text = match &first_mm {
                    Some(m) => format!(
                        "结论 {} · 第一个不一致: seq={} frame={} kind={} 记录={:.6} 重放={:?}",
                        rep.verdict, m.seq, m.frame, m.kind, m.recorded_metric, m.replayed_metric
                    ),
                    None => format!(
                        "结论 {} · 校验 {} 条 · 首错 frame={}",
                        rep.verdict,
                        rep.entries_checked,
                        rep.first_error.as_ref().map(|e| e.frame).unwrap_or(0)
                    ),
                };
                verdict.set_text(&text);
                state.borrow_mut().report = Some(rep);
                refresh();
            }
            glib::ControlFlow::Continue
        });
    }
    // 初始记录
    if let Some(p) = initial {
        if let Ok(rec) = record::load(&p) {
            let mut st = state.borrow_mut();
            st.record_path = Some(PathBuf::from(p));
            st.record = Some(rec);
            drop(st);
            refresh();
        }
    }
    let css = gtk4::CssProvider::new();
    css.load_from_data(".error { color: #c01c28; font-weight: bold; }");
    gtk4::style_context_add_provider_for_display(
        &gdk::Display::default().unwrap(),
        &css,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    window.present();
}
