use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, HeaderBar, Label, ListBox,
    Orientation, ScrolledWindow, TextView,
};

use crate::engine::{self, EngineEvent, RunConfig, RunOutcome};
use crate::model::Playlist;

const APP_ID: &str = "dev.plvcmp.app";

pub fn cmd(args: Vec<String>) -> Result<()> {
    let playlist = args
        .iter()
        .position(|a| a == "--playlist")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/demo/playlist_v1.json"));
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| {
        if let Err(e) = build_ui(app, &playlist) {
            eprintln!("ui error: {e:#}");
        }
    });
    // Parse our own flags above; hand GTK an empty argv so it does not
    // reject application-specific options.
    app.run_with_args(&[] as &[&str]);
    Ok(())
}

fn build_ui(app: &Application, playlist_path: &PathBuf) -> Result<()> {
    let playlist = Playlist::load(playlist_path)?;
    let out_dir = playlist_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("ui_run");

    let window = ApplicationWindow::builder()
        .application(app)
        .title(format!("plvcmp — {}", playlist.version))
        .default_width(720)
        .default_height(520)
        .build();

    let header = HeaderBar::new();
    let btn_start = Button::with_label("开始");
    let btn_pause = Button::with_label("暂停");
    let btn_resume = Button::with_label("继续");
    btn_pause.set_sensitive(false);
    btn_resume.set_sensitive(false);
    header.pack_start(&btn_start);
    header.pack_start(&btn_pause);
    header.pack_start(&btn_resume);
    window.set_titlebar(Some(&header));

    let list = ListBox::new();
    let mut rows = Vec::new();
    for e in &playlist.entries {
        let row = Label::new(Some(&format!("{} — pending", e.id)));
        row.set_halign(gtk4::Align::Start);
        list.append(&row);
        rows.push((e.id.clone(), row));
    }
    let rows = Rc::new(rows);
    let list_scroll = ScrolledWindow::builder().child(&list).build();

    let log = TextView::builder().editable(false).monospace(true).build();
    let log_scroll = ScrolledWindow::builder().child(&log).min_content_height(140).build();

    let vbox = GtkBox::new(Orientation::Vertical, 6);
    vbox.append(&list_scroll);
    vbox.append(&log_scroll);
    list_scroll.set_vexpand(true);
    window.set_child(Some(&vbox));

    let pause_flag: Rc<RefCell<Option<Arc<AtomicBool>>>> = Rc::new(RefCell::new(None));
    let (tx, rx) = async_channel::unbounded::<EngineEvent>();

    let log_ref = log.clone();
    let rows_ref = Rc::clone(&rows);
    let btn_pause_c = btn_pause.clone();
    let btn_resume_c = btn_resume.clone();
    glib::MainContext::default().spawn_local(async move {
        while let Ok(ev) = rx.recv().await {
            let buf = log_ref.buffer();
            match &ev {
                EngineEvent::EntryDone { result } => {
                    if let Some((_, row)) = rows_ref.iter().find(|(id, _)| id == &result.id) {
                        row.set_text(&format!(
                            "{} — {:?} ({} bytes)",
                            result.id, result.status, result.bytes_read
                        ));
                    }
                }
                EngineEvent::Paused { .. } => {
                    btn_pause_c.set_sensitive(false);
                    btn_resume_c.set_sensitive(true);
                }
                EngineEvent::Finished { .. } => {
                    btn_pause_c.set_sensitive(false);
                    btn_resume_c.set_sensitive(false);
                }
                _ => {}
            }
            let line = serde_json::to_string(&ev).unwrap();
            buf.insert(&mut buf.end_iter(), &format!("{line}\n"));
        }
    });

    let launch = {
        let playlist = playlist.clone();
        let out_dir = out_dir.clone();
        move |resume: bool, flag: Arc<AtomicBool>, tx: async_channel::Sender<EngineEvent>| {
            let playlist = playlist.clone();
            let out_dir = out_dir.clone();
            std::thread::spawn(move || {
                let cfg = RunConfig {
                    out_dir,
                    pause_after: None,
                    budget_bytes: u64::MAX,
                    pause_flag: Some(flag),
                };
                let tx2 = tx.clone();
                let outcome = engine::run(&playlist, &cfg, resume, move |ev| {
                    let _ = tx.try_send(ev);
                });
                if let Err(e) = outcome {
                    let _ = tx2.try_send(EngineEvent::Log {
                        msg: format!("engine error: {e:#}"),
                    });
                } else if let Ok(RunOutcome::Paused(_)) = outcome {
                    // Paused event already emitted by the engine.
                }
            });
        }
    };

    {
        let launch = launch.clone();
        let tx = tx.clone();
        let pause_flag = Rc::clone(&pause_flag);
        let btn_pause = btn_pause.clone();
        btn_start.connect_clicked(move |_| {
            let flag = Arc::new(AtomicBool::new(false));
            *pause_flag.borrow_mut() = Some(Arc::clone(&flag));
            btn_pause.set_sensitive(true);
            launch(false, flag, tx.clone());
        });
    }
    {
        let pause_flag = Rc::clone(&pause_flag);
        btn_pause.connect_clicked(move |_| {
            if let Some(flag) = pause_flag.borrow().as_ref() {
                flag.store(true, Ordering::SeqCst);
            }
        });
    }
    {
        let tx = tx.clone();
        let pause_flag = Rc::clone(&pause_flag);
        let btn_pause = btn_pause.clone();
        btn_resume.connect_clicked(move |btn| {
            let flag = Arc::new(AtomicBool::new(false));
            *pause_flag.borrow_mut() = Some(Arc::clone(&flag));
            btn.set_sensitive(false);
            btn_pause.set_sensitive(true);
            launch(true, flag, tx.clone());
        });
    }

    window.present();
    Ok(())
}
