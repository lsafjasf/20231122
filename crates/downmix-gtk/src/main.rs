//! GTK4 front end for the reviewable surround downmix flow.

use downmix_core::{
    clean_temp_state, discover::discover_objects, run_batch, verify_evidence, RunConfig,
};
use gtk4 as gtk;
use gtk::gdk::Display;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Debug, Clone)]
struct UiRow {
    object: String,
    unknown: String,
    status: String,
    detail: String,
}

#[derive(Debug, Clone)]
enum WorkerMessage {
    RunFinished {
        rows: Vec<UiRow>,
        summary: String,
    },
    RunFailed(String),
    VerifyFinished {
        rows: Vec<UiRow>,
        summary: String,
    },
    VerifyFailed(String),
}

#[derive(Default)]
struct AppState {
    input_dir: Option<PathBuf>,
    evidence_dir: Option<PathBuf>,
}

fn main() -> glib::ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let application = gtk::Application::builder()
        .application_id("org.reviewable.downmix")
        .build();
    application.connect_activate(build_ui);
    application.run()
}

fn build_ui(app: &gtk::Application) {
    let state = Rc::new(RefCell::new(AppState::default()));

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("环绕声对象下混 · 可复核流程")
        .default_width(980)
        .default_height(680)
        .build();

    let css = gtk::CssProvider::new();
    css.load_from_data(
        ".title { font-weight: 700; font-size: 15px; }
         .mono  { font-family: monospace; }
         .ok    { color: #1a7f37; }
         .fail  { color: #cf222e; }",
    );
    gtk::style_context_add_provider_for_display(
        &Display::default().expect("display"),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_top(14);
    root.set_margin_bottom(14);
    root.set_margin_start(14);
    root.set_margin_end(14);

    let header = gtk::Label::new(Some("环绕声对象下混 · 双版本快照证据台"));
    header.set_css_classes(&["title"]);
    header.set_halign(gtk::Align::Start);
    root.append(&header);

    let input_row = directory_row("输入对象目录", "选择输入目录");
    let evidence_row = directory_row("证据输出目录", "选择证据目录");
    root.append(&input_row.container);
    root.append(&evidence_row.container);

    state.borrow_mut().input_dir = Some(PathBuf::from("/tmp/dm-fixtures"));
    state.borrow_mut().evidence_dir = Some(PathBuf::from("/tmp/dm-evidence-gtk"));
    input_row
        .path_label
        .set_text("/tmp/dm-fixtures");
    evidence_row
        .path_label
        .set_text("/tmp/dm-evidence-gtk");

    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let run_button = gtk::Button::with_label("运行双版本下混");
    let verify_button = gtk::Button::with_label("复核证据");
    let clean_button = gtk::Button::with_label("清理临时状态");
    run_button.set_css_classes(&["suggested-action"]);
    button_row.append(&run_button);
    button_row.append(&verify_button);
    button_row.append(&clean_button);
    root.append(&button_row);

    let status = gtk::Label::new(Some("就绪"));
    status.set_halign(gtk::Align::Start);
    root.append(&status);

    let columns = vec![
        ("对象", 260),
        ("未知扩展", 90),
        ("状态", 100),
        ("说明", 420),
    ];
    let list: gtk::ColumnView = gtk::ColumnView::new(None::<gtk::NoSelection>);
    list.set_show_row_separators(true);
    list.set_show_column_separators(true);
    let store = gtk::gio::ListStore::new::<gtk::glib::Object>();
    let selection = gtk::NoSelection::new(Some(store.clone()));
    list.set_model(Some(&selection));
    for (title, width) in columns {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(move |_, list_item| {
            let item = list_item
                .downcast_ref::<gtk::ListItem>()
                .expect("ListItem");
            let label = gtk::Label::new(None);
            label.set_halign(gtk::Align::Start);
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            item.set_child(Some(&label));
        });
        let field = title.to_string();
        factory.connect_bind(move |_, list_item| {
            let item = list_item
                .downcast_ref::<gtk::ListItem>()
                .expect("ListItem");
            let row = item
                .item()
                .and_downcast::<RowObject>()
                .expect("RowObject");
            let label = item
                .child()
                .and_downcast::<gtk::Label>()
                .expect("label");
            label.set_text(&row.field(&field));
            label.set_css_classes(&["mono"]);
            if field == "状态" {
                match row.status().as_str() {
                    "completed" => label.set_css_classes(&["mono", "ok"]),
                    "failed" => label.set_css_classes(&["mono", "fail"]),
                    _ => label.set_css_classes(&["mono"]),
                }
            }
        });
        let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
        column.set_fixed_width(width);
        list.append_column(&column);
    }
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_vexpand(true);
    scroller.set_child(Some(&list));
    root.append(&scroller);

    window.set_child(Some(&root));

    wire_directory_picker(&input_row.button, &input_row.path_label, state.clone(), true);
    wire_directory_picker(
        &evidence_row.button,
        &evidence_row.path_label,
        state.clone(),
        false,
    );
    wire_run_button(
        &run_button,
        &status,
        &store,
        state.clone(),
    );
    wire_verify_button(&verify_button, &status, &store, state.clone());
    wire_clean_button(&clean_button, &status, state.clone());

    window.present();
}

struct DirectoryRow {
    container: gtk::Box,
    button: gtk::Button,
    path_label: gtk::Label,
}

fn directory_row(caption: &str, button_text: &str) -> DirectoryRow {
    let container = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let caption = gtk::Label::new(Some(caption));
    caption.set_width_chars(14);
    caption.set_halign(gtk::Align::Start);
    let button = gtk::Button::with_label(button_text);
    let path_label = gtk::Label::new(Some("（未选择）"));
    path_label.set_hexpand(true);
    path_label.set_halign(gtk::Align::Start);
    path_label.set_css_classes(&["mono"]);
    container.append(&caption);
    container.append(&button);
    container.append(&path_label);
    DirectoryRow {
        container,
        button,
        path_label,
    }
}

fn wire_directory_picker(
    button: &gtk::Button,
    label: &gtk::Label,
    state: Rc<RefCell<AppState>>,
    is_input: bool,
) {
    let label = label.clone();
    button.connect_clicked(move |_| {
        let dialog = gtk::FileDialog::builder()
            .title(if is_input { "选择输入目录" } else { "选择证据目录" })
            .modal(true)
            .build();
        let label_clone = label.clone();
        let state = state.clone();
        dialog.select_folder(
            None::<&gtk::Window>,
            None::<&gtk::gio::Cancellable>,
            move |result| {
                if let Ok(folder) = result {
                    if let Some(path) = folder.path() {
                        label_clone.set_text(&path.to_string_lossy());
                        let mut state = state.borrow_mut();
                        if is_input {
                            state.input_dir = Some(path);
                        } else {
                            state.evidence_dir = Some(path);
                        }
                    }
                }
            },
        );
    });
}

mod imp {
    use gtk4 as gtk;
    use gtk::glib::{self, prelude::*, subclass::prelude::*};
    use std::cell::RefCell;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::RowObject)]
    pub struct RowObject {
        #[property(get, set)]
        pub object: RefCell<String>,
        #[property(get, set)]
        pub unknown: RefCell<String>,
        #[property(get, set)]
        pub status: RefCell<String>,
        #[property(get, set)]
        pub detail: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RowObject {
        const NAME: &'static str = "DownmixRow";
        type Type = super::RowObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for RowObject {}
}

glib::wrapper! {
    pub struct RowObject(ObjectSubclass<imp::RowObject>);
}

impl RowObject {
    fn new(object: &str, unknown: &str, status: &str, detail: &str) -> Self {
        glib::Object::builder()
            .property("object", object)
            .property("unknown", unknown)
            .property("status", status)
            .property("detail", detail)
            .build()
    }

    fn field(&self, name: &str) -> String {
        match name {
            "对象" => self.object(),
            "未知扩展" => self.unknown(),
            "状态" => self.status(),
            "说明" => self.detail(),
            _ => String::new(),
        }
    }

}

fn set_busy(button: &gtk::Button, status: &gtk::Label, text: &str, busy: bool) {
    button.set_sensitive(!busy);
    status.set_text(text);
}

fn wire_run_button(
    button: &gtk::Button,
    status: &gtk::Label,
    store: &gtk::gio::ListStore,
    state: Rc<RefCell<AppState>>,
) {
    let store = store.clone();
    let status = status.clone();
    button.connect_clicked(move |btn| {
        let (input, evidence) = {
            let s = state.borrow();
            match (s.input_dir.clone(), s.evidence_dir.clone()) {
                (Some(i), Some(e)) => (i, e),
                _ => {
                    status.set_text("请先选择输入目录和证据目录");
                    return;
                }
            }
        };
        set_busy(btn, &status, "下混进行中……", true);
        store.remove_all();
        let btn = btn.clone();
        let store_worker = store.clone();
        let (tx, rx) = std::sync::mpsc::channel::<WorkerMessage>();
        let status_poll = status.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
            match rx.try_recv() {
                Ok(WorkerMessage::RunFinished { rows, summary }) => {
                    for row in &rows {
                        store_worker.append(&RowObject::new(
                            &row.object,
                            &row.unknown,
                            &row.status,
                            &row.detail,
                        ));
                    }
                    status_poll.set_text(&summary);
                    btn.set_sensitive(true);
                    glib::ControlFlow::Break
                }
                Ok(WorkerMessage::RunFailed(err)) => {
                    status_poll.set_text(&format!("批处理失败：{err}"));
                    btn.set_sensitive(true);
                    glib::ControlFlow::Break
                }
                Ok(_) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    btn.set_sensitive(true);
                    glib::ControlFlow::Break
                }
            }
        });
        std::thread::spawn(move || {
            let summary = run_batch(&RunConfig {
                input_root: input.clone(),
                evidence_root: evidence,
            });
            let _ = match summary {
                Ok(summary) => {
                    let rows = summary
                        .results
                        .iter()
                        .map(|result| UiRow {
                            object: result.path.to_string_lossy().into_owned(),
                            unknown: if result.unknown_extension {
                                "是".to_string()
                            } else {
                                "否".to_string()
                            },
                            status: format!("{:?}", result.status).to_lowercase(),
                            detail: result
                                .detail
                                .clone()
                                .unwrap_or_else(|| "双版本快照已留存".to_string()),
                        })
                        .collect();
                    tx.send(WorkerMessage::RunFinished {
                        rows,
                        summary: format!(
                            "完成：{} 成功 / {} 失败 / 共 {} 个对象",
                            summary.succeeded, summary.failed, summary.total_objects
                        ),
                    })
                }
                Err(err) => tx.send(WorkerMessage::RunFailed(err.to_string())),
            };
        });
    });
}

fn wire_verify_button(
    button: &gtk::Button,
    status: &gtk::Label,
    store: &gtk::gio::ListStore,
    state: Rc<RefCell<AppState>>,
) {
    let store = store.clone();
    let status = status.clone();
    button.connect_clicked(move |btn| {
        let (input, evidence) = {
            let s = state.borrow();
            match (s.input_dir.clone(), s.evidence_dir.clone()) {
                (Some(i), Some(e)) => (i, e),
                _ => {
                    status.set_text("请先选择输入目录和证据目录");
                    return;
                }
            }
        };
        set_busy(btn, &status, "复核进行中……", true);
        let btn = btn.clone();
        let store_worker = store.clone();
        let (tx, rx) = std::sync::mpsc::channel::<WorkerMessage>();
        let status_poll = status.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
            match rx.try_recv() {
                Ok(WorkerMessage::VerifyFinished { rows, summary }) => {
                    store_worker.remove_all();
                    for row in &rows {
                        store_worker.append(&RowObject::new(
                            &row.object,
                            &row.unknown,
                            &row.status,
                            &row.detail,
                        ));
                    }
                    status_poll.set_text(&summary);
                    btn.set_sensitive(true);
                    glib::ControlFlow::Break
                }
                Ok(WorkerMessage::VerifyFailed(err)) => {
                    status_poll.set_text(&format!("复核失败：{err}"));
                    btn.set_sensitive(true);
                    glib::ControlFlow::Break
                }
                Ok(_) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    btn.set_sensitive(true);
                    glib::ControlFlow::Break
                }
            }
        });
        std::thread::spawn(move || {
            let report = discover_objects(&input).and_then(|objects| {
                verify_evidence(&evidence, &objects)
            });
            let _ = match report {
                Ok(report) => {
                    let rows = report
                        .object_reports
                        .iter()
                        .map(|item| {
                            let detail = if item.ok {
                                "legacy/current 哈希与关联表均一致".to_string()
                            } else {
                                let mut problems = Vec::new();
                                problems.extend(item.legacy.problems.clone());
                                problems.extend(item.current.problems.clone());
                                problems.join("; ")
                            };
                            UiRow {
                                object: item.source_path.to_string_lossy().into_owned(),
                                unknown: "-".to_string(),
                                status: if item.ok {
                                    "completed".to_string()
                                } else {
                                    "failed".to_string()
                                },
                                detail,
                            }
                        })
                        .collect();
                    tx.send(WorkerMessage::VerifyFinished {
                        rows,
                        summary: if report.clean {
                            "复核通过：所有证据一致".to_string()
                        } else {
                            "复核发现不一致，详见列表".to_string()
                        },
                    })
                }
                Err(err) => tx.send(WorkerMessage::VerifyFailed(err.to_string())),
            };
        });
    });
}

fn wire_clean_button(
    button: &gtk::Button,
    status: &gtk::Label,
    state: Rc<RefCell<AppState>>,
) {
    let status = status.clone();
    button.connect_clicked(move |_btn| {
        let evidence = match state.borrow().evidence_dir.clone() {
            Some(path) => path,
            None => {
                status.set_text("请先选择证据目录");
                return;
            }
        };
        match clean_temp_state(&evidence) {
            Ok(removed) => status.set_text(&format!("已清理 {removed} 个临时目录")),
            Err(err) => status.set_text(&format!("清理失败：{err}")),
        }
    });
}
