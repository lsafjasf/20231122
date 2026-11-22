use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box as GtkBox, Button, Label, Orientation,
           ScrolledWindow, TextView};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::engine::{self, EngineConfig, RunOutcome};
use crate::state::{RunState, ANCHORS};

const APP_ID: &str = "dev.seamlessloop.App";
const UI_RUN_ID: &str = "ui-session";

struct UiState {
    paused: bool,
    busy: bool,
    evidence_offset: u64,
}

fn engine_config(ws: &std::path::Path, pause_after: Option<usize>) -> anyhow::Result<EngineConfig> {
    Ok(EngineConfig {
        input: ws.join("materials/original.wav"),
        pristine_sha256: crate::pristine_sha256(ws)?,
        run_root: ws.join("artifacts/runs"),
        run_id: UI_RUN_ID.into(),
        pause_after,
        simulate: Default::default(),
        needed_disk_mb: 32,
    })
}

fn next_anchor_index(ws: &std::path::Path) -> usize {
    RunState::load(&ws.join("artifacts/runs").join(UI_RUN_ID))
        .map(|s| s.next_anchor)
        .unwrap_or(0)
}

pub fn run() -> anyhow::Result<()> {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run_with_args(&["seamless-loop".to_string()]);
    Ok(())
}

fn build_ui(app: &Application) {
    let ws = crate::workspace();
    let state = Rc::new(RefCell::new(UiState { paused: false, busy: false, evidence_offset: 0 }));

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Seamless Audio Loop — 3-Anchor Pausable Pipeline")
        .default_width(820)
        .default_height(560)
        .build();

    let root = GtkBox::new(Orientation::Vertical, 8);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);

    let controls = GtkBox::new(Orientation::Horizontal, 8);
    let btn_start = Button::with_label("Start");
    let btn_pause = Button::with_label("Pause");
    let btn_resume = Button::with_label("Resume");
    let status = Label::new(Some("idle"));
    controls.append(&btn_start);
    controls.append(&btn_pause);
    controls.append(&btn_resume);
    controls.append(&status);
    root.append(&controls);

    let anchor_box = GtkBox::new(Orientation::Horizontal, 16);
    let anchor_labels: Vec<Label> = ANCHORS
        .iter()
        .map(|a| Label::new(Some(&format!("{a}: pending"))))
        .collect();
    for l in &anchor_labels {
        anchor_box.append(l);
    }
    root.append(&anchor_box);

    let log_view = TextView::builder().editable(false).monospace(true).build();
    let scroll = ScrolledWindow::builder().child(&log_view).vexpand(true).build();
    root.append(&scroll);
    window.set_child(Some(&root));

    // One engine invocation == one anchor stage. The pump chains stages
    // unless the user pressed Pause; Resume restarts the pump.
    let pump: Rc<dyn Fn()> = {
        let state = state.clone();
        let status = status.clone();
        let ws = ws.clone();
        Rc::new(move || {
            {
                let mut s = state.borrow_mut();
                if s.busy || s.paused {
                    return;
                }
                s.busy = true;
            }
            let next = next_anchor_index(&ws);
            if next >= ANCHORS.len() {
                status.set_text("completed");
                state.borrow_mut().busy = false;
                return;
            }
            status.set_text(&format!("running {}", ANCHORS[next]));
            let cfg = engine_config(&ws, Some(next));
            let state2 = state.clone();
            let status2 = status.clone();
            let (tx, rx) = std::sync::mpsc::channel::<String>();
            std::thread::spawn(move || {
                let msg = match cfg.and_then(|c| engine::run(&c)) {
                    Ok(RunOutcome::Paused { after_anchor, .. }) => {
                        format!("checkpoint {}", ANCHORS[after_anchor])
                    }
                    Ok(RunOutcome::Completed { .. }) => "completed".to_string(),
                    Err(e) => format!("error: {e}"),
                };
                let _ = tx.send(msg);
            });
            glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
                match rx.try_recv() {
                    Ok(msg) => {
                        status2.set_text(&msg);
                        state2.borrow_mut().busy = false;
                        glib::ControlFlow::Break
                    }
                    Err(_) => glib::ControlFlow::Continue,
                }
            });
        })
    };

    {
        let pump = pump.clone();
        btn_start.connect_clicked(move |_| pump());
    }
    {
        let state = state.clone();
        btn_pause.connect_clicked(move |_| state.borrow_mut().paused = true);
    }
    {
        let state = state.clone();
        let pump = pump.clone();
        btn_resume.connect_clicked(move |_| {
            state.borrow_mut().paused = false;
            pump();
        });
    }

    // Periodic refresh: anchor statuses from state.json, evidence tail, and
    // auto-advance to the next anchor when not paused.
    {
        let ws: PathBuf = ws.clone();
        let state = state.clone();
        let pump = pump.clone();
        let log_view = log_view.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(400), move || {
            if let Ok(s) = RunState::load(&ws.join("artifacts/runs").join(UI_RUN_ID)) {
                for (i, l) in anchor_labels.iter().enumerate() {
                    let a = &s.anchors[i];
                    let text = match a.status {
                        crate::state::AnchorStatus::Done => format!(
                            "{}: done ({} samples @ {:.3}s)",
                            a.name,
                            a.samples.unwrap_or(0),
                            a.stream_position_ns.unwrap_or(0) as f64 / 1e9
                        ),
                        crate::state::AnchorStatus::Pending => format!("{}: pending", a.name),
                    };
                    l.set_text(&text);
                }
            }
            let ev_path = ws.join("artifacts/runs").join(UI_RUN_ID).join("evidence.jsonl");
            if let Ok(text) = std::fs::read_to_string(&ev_path) {
                let mut off = state.borrow().evidence_offset as usize;
                if off > text.len() {
                    off = 0;
                }
                if text.len() > off {
                    let buf = log_view.buffer();
                    let mut end = buf.end_iter();
                    buf.insert(&mut end, &text[off..]);
                    state.borrow_mut().evidence_offset = text.len() as u64;
                }
            }
            let s = state.borrow();
            if !s.paused && !s.busy {
                drop(s);
                pump();
            }
            glib::ControlFlow::Continue
        });
    }

    window.present();
}
