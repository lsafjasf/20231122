use crate::experiment::load_scenario;
use crate::history::{History, StepStatus};
use crate::model::{Command, ExperimentSpec, StepSpec};
use anyhow::Result;
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

struct UiModel {
    spec: ExperimentSpec,
    history: History,
    request_counter: u64,
    last_request: Option<StepSpec>,
}

#[derive(Clone)]
struct UiView {
    revision: gtk::Label,
    clip: gtk::Label,
    state_hash: gtk::Label,
    status: gtk::Label,
    undo_list: gtk::ListBox,
    redo_list: gtk::ListBox,
}

struct UiButtons {
    trim: gtk::Button,
    gain: gtk::Button,
    undo: gtk::Button,
    redo: gtk::Button,
    duplicate: gtk::Button,
    replay: gtk::Button,
    reset: gtk::Button,
}

pub fn run(scenario_path: &Path, selftest: bool) -> Result<()> {
    let spec = load_scenario(scenario_path)?;
    let history = History::new(spec.controls.snapshot_v1.clip.clone());
    let model = Rc::new(RefCell::new(UiModel {
        spec,
        history,
        request_counter: 0,
        last_request: None,
    }));

    let app = gtk::Application::builder()
        .application_id("io.github.audio_undo_lab.replay")
        .build();
    app.connect_activate(move |app| {
        let window = build_window(app, model.clone());
        window.present();
        if selftest {
            let app = app.clone();
            gtk::glib::timeout_add_seconds_local(1, move || {
                println!("GTK4_UI_READY");
                app.quit();
                gtk::glib::ControlFlow::Break
            });
        }
    });
    app.run_with_args::<&str>(&[]);
    Ok(())
}

fn build_window(app: &gtk::Application, model: Rc<RefCell<UiModel>>) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Audio Undo Replay Lab")
        .default_width(980)
        .default_height(640)
        .build();

    let header = gtk::HeaderBar::new();
    let title = gtk::Label::new(Some("Audio Undo Replay Lab"));
    header.set_title_widget(Some(&title));
    window.set_titlebar(Some(&header));

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);

    let intro = gtk::Label::new(Some(
        "Replay .undoexp steps, inspect state hashes, and submit the same request twice.",
    ));
    intro.set_xalign(0.0);
    root.append(&intro);

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_wide_handle(true);
    root.append(&paned);

    let controls = gtk::Box::new(gtk::Orientation::Vertical, 8);
    controls.set_margin_end(14);
    paned.set_start_child(Some(&controls));

    let view = UiView {
        revision: gtk::Label::new(None),
        clip: gtk::Label::new(None),
        state_hash: gtk::Label::new(None),
        status: gtk::Label::new(Some("Ready")),
        undo_list: gtk::ListBox::new(),
        redo_list: gtk::ListBox::new(),
    };
    for label in [&view.revision, &view.clip, &view.state_hash] {
        label.set_xalign(0.0);
        label.set_selectable(true);
        label.set_wrap(true);
        controls.append(label);
    }

    let buttons = UiButtons {
        trim: gtk::Button::with_label("Trim 200-1800 ms"),
        gain: gtk::Button::with_label("Gain 70%"),
        undo: gtk::Button::with_label("Undo"),
        redo: gtk::Button::with_label("Redo"),
        duplicate: gtk::Button::with_label("Duplicate last request"),
        replay: gtk::Button::with_label("Replay canonical"),
        reset: gtk::Button::with_label("Reset to snapshot v1"),
    };

    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    button_row.append(&buttons.trim);
    button_row.append(&buttons.gain);
    controls.append(&button_row);

    let history_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    history_row.append(&buttons.undo);
    history_row.append(&buttons.redo);
    controls.append(&history_row);

    let replay_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    replay_row.append(&buttons.duplicate);
    replay_row.append(&buttons.replay);
    controls.append(&replay_row);

    controls.append(&buttons.reset);
    view.status.set_xalign(0.0);
    view.status.set_wrap(true);
    controls.append(&view.status);

    let stacks = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let undo_label = gtk::Label::new(Some("Undo stack (newest first)"));
    undo_label.set_xalign(0.0);
    stacks.append(&undo_label);
    let undo_scroll = gtk::ScrolledWindow::new();
    undo_scroll.set_child(Some(&view.undo_list));
    undo_scroll.set_vexpand(true);
    stacks.append(&undo_scroll);
    let redo_label = gtk::Label::new(Some("Redo stack"));
    redo_label.set_xalign(0.0);
    stacks.append(&redo_label);
    stacks.append(&view.redo_list);
    paned.set_end_child(Some(&stacks));
    paned.set_position(430);

    install_actions(&buttons, &model, &view);
    view.refresh(&model.borrow());
    window.set_child(Some(&root));
    window
}

fn install_actions(buttons: &UiButtons, model: &Rc<RefCell<UiModel>>, view: &UiView) {
    connect_step(
        &buttons.trim,
        model,
        view,
        Command::Trim {
            start_ms: 200,
            end_ms: 1800,
        },
    );
    connect_step(
        &buttons.gain,
        model,
        view,
        Command::Gain { gain_milli: 700 },
    );
    connect_step(&buttons.undo, model, view, Command::Undo);
    connect_step(&buttons.redo, model, view, Command::Redo);

    let model_clone = model.clone();
    let view_clone = view.clone();
    buttons.duplicate.connect_clicked(move |_| {
        let mut model = model_clone.borrow_mut();
        if let Some(previous) = model.last_request.clone() {
            let result = model.history.apply_step(&previous);
            model.last_request = Some(previous.clone());
            view_clone.status.set_text(&format!(
                "Duplicate {}: {}",
                previous.request_id,
                status_name(&result.status)
            ));
        } else {
            view_clone.status.set_text("No request to duplicate");
        }
        view_clone.refresh(&model);
    });

    let model_clone = model.clone();
    let view_clone = view.clone();
    buttons.replay.connect_clicked(move |_| {
        let mut model = model_clone.borrow_mut();
        let canonical = model.spec.objects.first().cloned();
        model.history = History::new(model.spec.controls.snapshot_v1.clip.clone());
        model.last_request = None;
        if let Some(object) = canonical {
            let object_id = object.id.clone();
            let mut final_status = "empty";
            for step in object.steps {
                let result = model.history.apply_step(&step);
                model.last_request = Some(step);
                final_status = status_name(&result.status);
            }
            view_clone.status.set_text(&format!(
                "Replayed {} ({} steps): {final_status}",
                object_id,
                model.history.revision()
            ));
        }
        view_clone.refresh(&model);
    });

    let model_clone = model.clone();
    let view_clone = view.clone();
    buttons.reset.connect_clicked(move |_| {
        let mut model = model_clone.borrow_mut();
        model.history = History::new(model.spec.controls.snapshot_v1.clip.clone());
        model.last_request = None;
        view_clone.status.set_text("Reset to snapshot v1");
        view_clone.refresh(&model);
    });
}

fn connect_step(
    button: &gtk::Button,
    model: &Rc<RefCell<UiModel>>,
    view: &UiView,
    command: Command,
) {
    let model = model.clone();
    let view = view.clone();
    button.connect_clicked(move |_| {
        let mut model = model.borrow_mut();
        model.request_counter += 1;
        let step = StepSpec {
            request_id: format!("ui-request-{:03}", model.request_counter),
            base_revision: Some(model.history.revision()),
            command: command.clone(),
        };
        let result = model.history.apply_step(&step);
        let status = status_name(&result.status);
        let message = result.message.clone();
        model.last_request = Some(step);
        view.status.set_text(&format!(
            "{status}{}",
            message
                .map(|message| format!(": {message}"))
                .unwrap_or_default()
        ));
        view.refresh(&model);
    });
}

impl UiView {
    fn refresh(&self, model: &UiModel) {
        let state = model.history.state();
        self.revision
            .set_text(&format!("Revision: {}", model.history.revision()));
        self.clip.set_text(&format!(
            "Clip: {}  trim {}..{} ms  gain {}%",
            state.clip_id,
            state.trim_start_ms,
            state.trim_end_ms,
            state.gain_milli / 10
        ));
        self.state_hash
            .set_text(&format!("State: {}", model.history.state_hash()));
        while let Some(child) = self.undo_list.first_child() {
            self.undo_list.remove(&child);
        }
        for entry in model.history.undo_entries().iter().rev() {
            self.undo_list.append(&gtk::Label::new(Some(&format!(
                "#{:02} {}",
                entry.sequence, entry.submission.request_id
            ))));
        }
        while let Some(child) = self.redo_list.first_child() {
            self.redo_list.remove(&child);
        }
        for entry in model.history.redo_entries().iter().rev() {
            self.redo_list.append(&gtk::Label::new(Some(&format!(
                "#{:02} {}",
                entry.sequence, entry.submission.request_id
            ))));
        }
    }
}

fn status_name(status: &StepStatus) -> &'static str {
    match status {
        StepStatus::Applied => "applied",
        StepStatus::Undone => "undone",
        StepStatus::Redone => "redone",
        StepStatus::DuplicateIgnored => "duplicate ignored",
        StepStatus::Rejected => "rejected",
    }
}
