use crate::acceptance::AcceptanceReport;
use crate::checkpoint::{Checkpoint, JobState};
use crate::fixture::load_profile;
use crate::metadata::FrameMetadata;
use crate::model::{CameraProfile, METADATA_RECORD_BYTES, RepairConfig};
use crate::session::{ProgressEvent, RunOptions, RunOutcome, Session};
use gtk::gio::prelude::FileExt as _;
use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;
use std::cell::RefCell;
use std::fs::{self, File};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TARGET_FRAME: u32 = 72;
const TARGET_PTS_NS: u64 = 2_400_000_000;
const TARGET_VIDEO_OFFSET: u64 = 0x144000;
const TARGET_METADATA_OFFSET: u64 = 0x1200;

#[allow(clippy::large_enum_variant)]
enum WorkerMsg {
    Progress(ProgressEvent),
    Finished(Result<RunOutcome, String>),
}

struct UiState {
    fixture_dir: PathBuf,
    run_root: PathBuf,
    session: Option<Session>,
    receiver: Option<Receiver<WorkerMsg>>,
    pause_flag: Arc<AtomicBool>,
    running: bool,
}

struct AppUi {
    window: gtk::ApplicationWindow,
    source_value: gtk::Label,
    model_value: gtk::Label,
    hash_value: gtk::Label,
    extreme_value: gtk::Label,
    object_value: gtk::Label,
    workspace_value: gtk::Label,
    preview: gtk::Picture,
    preview_caption: gtk::Label,
    state_label: gtk::Label,
    detail_label: gtk::Label,
    progress: gtk::ProgressBar,
    chunk_spin: gtk::SpinButton,
    budget_spin: gtk::SpinButton,
    start_button: gtk::Button,
    pause_button: gtk::Button,
    resume_button: gtk::Button,
    checkpoint_button: gtk::Button,
    low_button: gtk::Button,
    blocked_button: gtk::Button,
    open_button: gtk::Button,
    new_button: gtk::Button,
    evidence_list: gtk::ListBox,
    acceptance_state: gtk::Label,
    acceptance_time: gtk::Label,
    acceptance_frame: gtk::Label,
    acceptance_video_offset: gtk::Label,
    acceptance_meta_offset: gtk::Label,
    acceptance_before: gtk::Label,
    acceptance_after: gtk::Label,
    state: RefCell<UiState>,
}

pub fn run() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("io.github.hdrrepair.lab")
        .build();
    app.connect_activate(build_window);
    app.run()
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        r#"
        window { background-color: #0d1218; color: #e8eef6; }
        headerbar { background: #151c25; border-bottom: 1px solid #283340; }
        .title-main { font-size: 18px; font-weight: 800; letter-spacing: 0.4px; }
        .title-sub { color: #8fa2b8; font-size: 11px; }
        .card { background: #151c25; border: 1px solid #26313e; border-radius: 12px; padding: 14px; }
        .card-title { color: #93a7bd; font-size: 11px; font-weight: 700; letter-spacing: 0.8px; }
        .value { color: #f1f6fb; font-size: 13px; }
        .muted { color: #8496aa; font-size: 11px; }
        .mono { font-family: monospace; font-size: 11px; }
        .hash { color: #63d4b0; font-family: monospace; font-size: 10px; }
        .accent { color: #64d9b6; }
        .warn { color: #ffbb66; }
        .danger { color: #ff7b87; }
        .big-value { color: #ffffff; font-size: 22px; font-weight: 800; }
        .evidence-row { background: #111820; border-radius: 8px; padding: 9px; margin: 3px 0; }
        button { border-radius: 8px; padding: 7px 12px; }
        button.suggested-action { background: #38bd96; color: #06120f; font-weight: 700; }
        button.destructive-action { background: #bf4d5a; color: white; }
        progressbar trough { background: #202b38; border-radius: 6px; min-height: 10px; }
        progressbar progress { background: #54d4ad; border-radius: 6px; min-height: 10px; }
        picture { background: #080c11; border-radius: 9px; }
        notebook, textview { background: #0d1218; color: #c8d6e4; }
        "#,
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn build_window(app: &gtk::Application) {
    install_css();
    let fixture_dir = std::env::var_os("HDR_REPAIR_FIXTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("fixtures/camera-extreme"));
    let run_root = std::env::var_os("HDR_REPAIR_RUN_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/runs"));
    let (source_value, model_value, hash_value, extreme_value, object_value, workspace_value) =
        build_source_labels();
    let (preview, preview_caption) = build_preview();
    let (state_label, detail_label, progress) = build_progress();
    let (chunk_spin, budget_spin) = build_controls();
    let start_button = gtk::Button::with_label("开始处理");
    let pause_button = gtk::Button::with_label("暂停");
    let resume_button = gtk::Button::with_label("继续");
    let checkpoint_button = gtk::Button::with_label("在 2.400s 暂停");
    let low_button = gtk::Button::with_label("80 KiB 降级");
    let blocked_button = gtk::Button::with_label("40 KiB 不足");
    let open_button = gtk::Button::with_label("选择夹具");
    let new_button = gtk::Button::with_label("新任务");
    start_button.add_css_class("suggested-action");
    checkpoint_button.add_css_class("suggested-action");
    blocked_button.add_css_class("destructive-action");
    let evidence_list = gtk::ListBox::new();
    evidence_list.set_selection_mode(gtk::SelectionMode::None);
    let (
        acceptance_state,
        acceptance_time,
        acceptance_frame,
        acceptance_video_offset,
        acceptance_meta_offset,
        acceptance_before,
        acceptance_after,
    ) = build_acceptance_labels();

    let ui = Rc::new(AppUi {
        window: gtk::ApplicationWindow::builder()
            .application(app)
            .title("HDR 元数据修复台")
            .default_width(1320)
            .default_height(860)
            .build(),
        source_value,
        model_value,
        hash_value,
        extreme_value,
        object_value,
        workspace_value,
        preview,
        preview_caption,
        state_label,
        detail_label,
        progress,
        chunk_spin,
        budget_spin,
        start_button,
        pause_button,
        resume_button,
        checkpoint_button,
        low_button,
        blocked_button,
        open_button,
        new_button,
        evidence_list,
        acceptance_state,
        acceptance_time,
        acceptance_frame,
        acceptance_video_offset,
        acceptance_meta_offset,
        acceptance_before,
        acceptance_after,
        state: RefCell::new(UiState {
            fixture_dir,
            run_root,
            session: None,
            receiver: None,
            pause_flag: Arc::new(AtomicBool::new(false)),
            running: false,
        }),
    });
    ui.build_layout();
    ui.bind_actions();
    ui.refresh_fixture();
    let weak = Rc::downgrade(&ui);
    glib::timeout_add_local(Duration::from_millis(50), move || {
        if let Some(ui) = weak.upgrade() {
            ui.poll_worker();
            glib::ControlFlow::Continue
        } else {
            glib::ControlFlow::Break
        }
    });
    ui.window.present();
}

fn plain_label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.add_css_class(class);
    label
}

fn card(title: &str) -> (gtk::Box, gtk::Box) {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 9);
    outer.add_css_class("card");
    outer.append(&plain_label(title, "card-title"));
    let body = gtk::Box::new(gtk::Orientation::Vertical, 7);
    outer.append(&body);
    (outer, body)
}

fn row(label: &str, value: &gtk::Label) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let key = plain_label(label, "muted");
    key.set_width_chars(13);
    value.set_hexpand(true);
    row.append(&key);
    row.append(value);
    row
}

fn build_source_labels() -> (
    gtk::Label,
    gtk::Label,
    gtk::Label,
    gtk::Label,
    gtk::Label,
    gtk::Label,
) {
    (
        plain_label("尚未载入", "value"),
        plain_label("-", "value"),
        plain_label("-", "hash"),
        plain_label("-", "mono"),
        plain_label("-", "mono"),
        plain_label("-", "mono"),
    )
}

fn build_preview() -> (gtk::Picture, gtk::Label) {
    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_size_request(560, 290);
    let caption = plain_label("等待处理帧", "muted");
    (picture, caption)
}

fn build_progress() -> (gtk::Label, gtk::Label, gtk::ProgressBar) {
    let state = plain_label("就绪", "big-value");
    let detail = plain_label(
        "可暂停、恢复，并在 2.400s 验收点比较修复前后元数据。",
        "muted",
    );
    let progress = gtk::ProgressBar::new();
    progress.set_show_text(false);
    (state, detail, progress)
}

fn build_controls() -> (gtk::SpinButton, gtk::SpinButton) {
    let chunk = gtk::SpinButton::with_range(1.0, 120.0, 1.0);
    chunk.set_value(8.0);
    chunk.set_hexpand(true);
    let budget = gtk::SpinButton::with_range(32.0, 1_048_576.0, 32.0);
    budget.set_value(1024.0);
    budget.set_hexpand(true);
    (chunk, budget)
}

fn build_acceptance_labels() -> (
    gtk::Label,
    gtk::Label,
    gtk::Label,
    gtk::Label,
    gtk::Label,
    gtk::Label,
    gtk::Label,
) {
    let time = format!("{:.9} s", TARGET_PTS_NS as f64 / 1_000_000_000.0);
    let frame = format!("帧 {TARGET_FRAME} / pts {TARGET_PTS_NS} ns");
    let video = format!("视频原始偏移 0x{TARGET_VIDEO_OFFSET:X} ({TARGET_VIDEO_OFFSET})");
    let metadata =
        format!("元数据原始偏移 0x{TARGET_METADATA_OFFSET:X} ({TARGET_METADATA_OFFSET})");
    (
        plain_label("等待处理", "warn"),
        plain_label(&time, "big-value"),
        plain_label(&frame, "value"),
        plain_label(&video, "mono"),
        plain_label(&metadata, "mono"),
        plain_label("CLL 65535 / FALL 65535 / 白点 0,0", "danger"),
        plain_label("尚未生成", "muted"),
    )
}

fn read_metadata(path: &PathBuf, offset: u64) -> Option<FrameMetadata> {
    let mut bytes = [0u8; METADATA_RECORD_BYTES];
    File::open(path)
        .ok()?
        .read_exact_at(&mut bytes, offset)
        .ok()?;
    FrameMetadata::decode(&bytes).ok()
}

fn load_preview_frame(video: &PathBuf, profile: &CameraProfile, frame: u32) -> Option<Vec<u8>> {
    let width = profile.width as usize;
    let height = profile.height as usize;
    let mut gray = vec![0u8; width * height * 2];
    File::open(video)
        .ok()?
        .read_exact_at(&mut gray, profile.video_offset(frame))
        .ok()?;
    let mut rgba = Vec::with_capacity(width * height * 4);
    for pixel in gray.as_chunks::<2>().0 {
        let value = u16::from_le_bytes([pixel[0], pixel[1]]);
        let byte = (value >> 8) as u8;
        rgba.extend_from_slice(&[byte, byte, byte, 255]);
    }
    Some(rgba)
}

fn describe_metadata(meta: &FrameMetadata) -> String {
    format!(
        "CLL {} / FALL {} / peak {} / white {},{} / mastering {}:{}",
        meta.max_cll,
        meta.max_fall,
        meta.max_nits,
        meta.white_point[0],
        meta.white_point[1],
        meta.mastering_max_nits,
        meta.mastering_min_nits
    )
}

fn workspace_name(root: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    root.join(format!("ui-{stamp}"))
}

fn clear_list(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

impl AppUi {
    fn build_layout(self: &Rc<Self>) {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let header = gtk::HeaderBar::new();
        let title_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        title_box.append(&plain_label("HDR 元数据修复台", "title-main"));
        title_box.append(&plain_label(
            "Rust · GStreamer · GTK4  /  pause-safe metadata repair",
            "title-sub",
        ));
        header.pack_start(&title_box);
        header.pack_end(&self.open_button);
        header.pack_end(&self.new_button);
        root.append(&header);

        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_wide_handle(true);
        paned.set_vexpand(true);
        let left_scroll = gtk::ScrolledWindow::new();
        left_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        left_scroll.set_width_request(405);
        let left = gtk::Box::new(gtk::Orientation::Vertical, 11);
        left.set_margin_top(14);
        left.set_margin_bottom(14);
        left.set_margin_start(14);
        left.set_margin_end(10);

        let (source_card, source_body) = card("原始材料 / IMMUTABLE ORIGINAL");
        source_body.append(&row("夹具", &self.source_value));
        source_body.append(&row("相机", &self.model_value));
        source_body.append(&row("SHA-256", &self.hash_value));
        source_body.append(&row("工作区", &self.workspace_value));
        left.append(&source_card);

        let (extreme_card, extreme_body) = card("极限参数 / XTREME-65535");
        extreme_body.append(&self.extreme_value);
        let object_title = plain_label("重复对象", "muted");
        extreme_body.append(&object_title);
        extreme_body.append(&self.object_value);
        left.append(&extreme_card);

        let (control_card, control_body) = card("处理控制");
        let chunk_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        chunk_row.append(&plain_label("块大小（帧）", "muted"));
        chunk_row.append(&self.chunk_spin);
        control_body.append(&chunk_row);
        let budget_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        budget_row.append(&plain_label("内存预算（KiB）", "muted"));
        budget_row.append(&self.budget_spin);
        control_body.append(&budget_row);
        let action_grid = gtk::Grid::new();
        action_grid.set_row_spacing(7);
        action_grid.set_column_spacing(7);
        action_grid.attach(&self.start_button, 0, 0, 1, 1);
        action_grid.attach(&self.pause_button, 1, 0, 1, 1);
        action_grid.attach(&self.resume_button, 0, 1, 1, 1);
        action_grid.attach(&self.checkpoint_button, 1, 1, 1, 1);
        action_grid.attach(&self.low_button, 0, 2, 1, 1);
        action_grid.attach(&self.blocked_button, 1, 2, 1, 1);
        control_body.append(&action_grid);
        left.append(&control_card);

        let (acceptance_card, acceptance_body) = card("验收点 / 02.400 s");
        acceptance_body.append(&self.acceptance_state);
        acceptance_body.append(&self.acceptance_time);
        acceptance_body.append(&self.acceptance_frame);
        acceptance_body.append(&self.acceptance_video_offset);
        acceptance_body.append(&self.acceptance_meta_offset);
        acceptance_body.append(&plain_label("BEFORE", "card-title"));
        acceptance_body.append(&self.acceptance_before);
        acceptance_body.append(&plain_label("AFTER", "card-title"));
        acceptance_body.append(&self.acceptance_after);
        left.append(&acceptance_card);
        left_scroll.set_child(Some(&left));
        paned.set_start_child(Some(&left_scroll));

        let right = gtk::Box::new(gtk::Orientation::Vertical, 11);
        right.set_margin_top(14);
        right.set_margin_bottom(14);
        right.set_margin_start(10);
        right.set_margin_end(14);
        let (preview_card, preview_body) = card("GSTREAMER 帧预览");
        preview_body.append(&self.preview_caption);
        preview_body.append(&self.preview);
        preview_card.set_vexpand(true);
        right.append(&preview_card);

        let (progress_card, progress_body) = card("运行状态");
        progress_body.append(&self.state_label);
        progress_body.append(&self.detail_label);
        progress_body.append(&self.progress);
        right.append(&progress_card);

        let (evidence_card, evidence_body) = card("关键证据 / EVENTS");
        let evidence_scroll = gtk::ScrolledWindow::new();
        evidence_scroll.set_vexpand(true);
        evidence_scroll.set_min_content_height(180);
        evidence_scroll.set_child(Some(&self.evidence_list));
        evidence_body.append(&evidence_scroll);
        evidence_card.set_vexpand(true);
        right.append(&evidence_card);
        paned.set_end_child(Some(&right));
        root.append(&paned);
        self.window.set_child(Some(&root));
    }

    fn bind_actions(self: &Rc<Self>) {
        let ui = self.clone();
        self.start_button.connect_clicked(move |_| {
            ui.start_run(false, None, None);
        });
        let ui = self.clone();
        self.pause_button.connect_clicked(move |_| {
            let running = ui.state.borrow().running;
            if running {
                ui.state.borrow().pause_flag.store(true, Ordering::SeqCst);
                ui.state_label.set_text("暂停中…等待安全边界");
                ui.detail_label
                    .set_text("当前块会完整提交并原子保存检查点，然后暂停。");
            }
        });
        let ui = self.clone();
        self.resume_button.connect_clicked(move |_| {
            ui.state.borrow().pause_flag.store(false, Ordering::SeqCst);
            ui.start_run(false, Some(None), None);
        });
        let ui = self.clone();
        self.checkpoint_button.connect_clicked(move |_| {
            ui.start_run(true, Some(Some(TARGET_FRAME)), None);
        });
        let ui = self.clone();
        self.low_button.connect_clicked(move |_| {
            ui.budget_spin.set_value(80.0);
            ui.start_run(false, None, Some(80 * 1024));
        });
        let ui = self.clone();
        self.blocked_button.connect_clicked(move |_| {
            ui.budget_spin.set_value(40.0);
            ui.start_run(false, None, Some(40 * 1024));
        });
        let ui = self.clone();
        self.open_button.connect_clicked(move |_| {
            ui.choose_fixture();
        });
        let ui = self.clone();
        self.new_button.connect_clicked(move |_| {
            ui.state.borrow_mut().session = None;
            clear_list(&ui.evidence_list);
            ui.acceptance_state.set_text("等待处理");
            ui.acceptance_state.remove_css_class("accent");
            ui.acceptance_state.add_css_class("warn");
            ui.refresh_fixture();
            ui.add_evidence(
                "新任务",
                "已清空当前会话；下一次开始会在新工作区复制只读原始件。",
            );
        });
    }

    fn refresh_fixture(&self) {
        let fixture = self.state.borrow().fixture_dir.clone();
        let session_profile = self
            .state
            .borrow()
            .session
            .as_ref()
            .map(|session| session.manifest.config.profile.clone());
        let profile = if let Some(profile) = session_profile {
            profile
        } else {
            match load_profile(&fixture.join("camera.json")) {
                Ok(profile) => profile,
                Err(error) => {
                    self.state_label.set_text("夹具不可用");
                    self.detail_label.set_text(&format!("{error:#}"));
                    return;
                }
            }
        };
        let session_info = {
            let state = self.state.borrow();
            state.session.as_ref().map(|session| {
                (
                    session.paths.root.clone(),
                    session.paths.video.clone(),
                    session.paths.metadata.clone(),
                    session.paths.repaired_metadata.clone(),
                    session.checkpoint.clone(),
                )
            })
        };
        let (workspace, video, metadata, repaired, checkpoint) = if let Some(info) = session_info {
            info
        } else {
            (
                PathBuf::new(),
                fixture.join("video.gray16"),
                fixture.join("metadata.bin"),
                fixture.join("metadata.bin"),
                Checkpoint::prepared(
                    "preview",
                    &profile,
                    profile.source_video_sha256.clone(),
                    String::new(),
                ),
            )
        };
        self.source_value.set_text(&fixture.display().to_string());
        self.model_value.set_text(&profile.camera_model);
        self.hash_value.set_text(&profile.source_video_sha256);
        self.extreme_value.set_text(&format!(
            "CLL {} · FALL {} · peak {} · white {},{} · mastering {}/{}",
            profile.fault_profile.max_cll,
            profile.fault_profile.max_fall,
            profile.fault_profile.peak_nits,
            profile.fault_profile.white_point[0],
            profile.fault_profile.white_point[1],
            profile.fault_profile.mastering_max_nits,
            profile.fault_profile.mastering_min_nits
        ));
        self.object_value.set_text(&format!(
            "{} · bbox [{},{} {}×{}] · frames {:?}",
            profile.repeated_object.id,
            profile.repeated_object.rect.x,
            profile.repeated_object.rect.y,
            profile.repeated_object.rect.width,
            profile.repeated_object.rect.height,
            profile.repeated_object.frames
        ));
        self.workspace_value
            .set_text(if workspace.as_os_str().is_empty() {
                "尚未创建"
            } else {
                workspace.to_str().unwrap_or("-")
            });
        if let Some(before) = read_metadata(&metadata, profile.metadata_offset(TARGET_FRAME)) {
            self.acceptance_before.set_text(&describe_metadata(&before));
        }
        if checkpoint.next_frame > TARGET_FRAME
            && let Some(after) = read_metadata(&repaired, profile.metadata_offset(TARGET_FRAME))
            && after.is_repaired()
        {
            self.acceptance_after.set_text(&describe_metadata(&after));
        }
        let preview_frame = if checkpoint.next_frame > TARGET_FRAME {
            TARGET_FRAME
        } else {
            0
        };
        if let Some(rgba) = load_preview_frame(&video, &profile, preview_frame) {
            self.set_preview(&rgba, &profile);
            self.preview_caption.set_text(&format!(
                "frame {} · pts {:.3}s · {}×{} · GRAY16_LE",
                preview_frame,
                profile.pts_ns(preview_frame) as f64 / 1_000_000_000.0,
                profile.width,
                profile.height
            ));
        }
        let (running, can_resume) = {
            let state = self.state.borrow();
            (
                state.running,
                state.session.as_ref().is_some_and(|session| {
                    matches!(
                        session.checkpoint.state,
                        JobState::Paused | JobState::ResourcePaused
                    )
                }),
            )
        };
        self.start_button.set_sensitive(!running);
        self.pause_button.set_sensitive(running);
        self.resume_button.set_sensitive(!running && can_resume);
        self.checkpoint_button.set_sensitive(!running);
        self.low_button.set_sensitive(!running);
        self.blocked_button.set_sensitive(!running);
        if !running {
            self.progress.set_fraction(
                f64::from(checkpoint.next_frame) / f64::from(profile.frame_count.max(1)),
            );
            match checkpoint.state {
                JobState::Prepared => {
                    self.state_label.set_text("就绪");
                    self.detail_label
                        .set_text("原始件仍保持只读；开始后将按块处理并在安全边界保存检查点。");
                }
                JobState::Paused => {
                    self.state_label.set_text("已暂停");
                    self.detail_label.set_text(&format!(
                        "下一帧 {} · pts {} ns · 视频偏移 0x{:X} · 元数据偏移 0x{:X}",
                        checkpoint.next_frame,
                        checkpoint.next_pts_ns,
                        checkpoint.next_video_offset,
                        checkpoint.next_metadata_offset
                    ));
                }
                JobState::ResourcePaused => {
                    self.state_label.set_text("资源不足，已安全暂停");
                    self.detail_label.set_text(&format!(
                        "停在帧 {} / pts {} ns；提高预算后可从原始偏移 0x{:X} 恢复。",
                        checkpoint.next_frame, checkpoint.next_pts_ns, checkpoint.next_video_offset
                    ));
                }
                JobState::Complete => {
                    self.state_label.set_text("修复完成");
                    self.detail_label.set_text(
                        "可检查 acceptance.json、events.jsonl 与 key_frames 原始字节证据。",
                    );
                }
                JobState::Running => {
                    self.state_label.set_text("运行中");
                }
            }
        }
    }

    fn set_preview(&self, rgba: &[u8], profile: &CameraProfile) {
        let width = profile.width as i32;
        let height = profile.height as i32;
        let expected = width as usize * height as usize * 4;
        if rgba.len() < expected {
            return;
        }
        let bytes = glib::Bytes::from(&rgba[..expected]);
        let texture = gdk::MemoryTexture::new(
            width,
            height,
            gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            width as usize * 4,
        );
        self.preview.set_paintable(Some(&texture));
    }

    fn add_evidence(&self, title: &str, detail: &str) {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 3);
        row.add_css_class("evidence-row");
        row.append(&plain_label(title, "value"));
        let detail_label = plain_label(detail, "mono");
        detail_label.set_selectable(true);
        row.append(&detail_label);
        self.evidence_list.prepend(&row);
    }

    fn start_run(
        self: &Rc<Self>,
        pause_at_acceptance: bool,
        pause_override: Option<Option<u32>>,
        budget_override: Option<u64>,
    ) {
        if self.state.borrow().running {
            return;
        }
        let fixture = self.state.borrow().fixture_dir.clone();
        let session_profile = self
            .state
            .borrow()
            .session
            .as_ref()
            .map(|session| session.manifest.config.profile.clone());
        let profile = if let Some(profile) = session_profile {
            profile
        } else {
            match load_profile(&fixture.join("camera.json")) {
                Ok(profile) => profile,
                Err(error) => {
                    self.add_evidence("无法开始", &format!("{error:#}"));
                    return;
                }
            }
        };
        let budget = budget_override
            .unwrap_or_else(|| (self.budget_spin.value() as u64).saturating_mul(1024));
        let chunk_frames = self.chunk_spin.value().max(1.0) as u32;
        let needs_new = {
            let state = self.state.borrow();
            state
                .session
                .as_ref()
                .is_none_or(|session| session.checkpoint.state == JobState::Complete)
        };
        if needs_new {
            let run_root = self.state.borrow().run_root.clone();
            if let Err(error) = fs::create_dir_all(&run_root) {
                self.add_evidence("工作区创建失败", &format!("{error:#}"));
                return;
            }
            let workspace = workspace_name(&run_root);
            let mut config = RepairConfig::demo(profile);
            config.chunk_frames = chunk_frames;
            config.memory_budget_bytes = budget;
            config.pause_before_frame = pause_at_acceptance.then_some(TARGET_FRAME);
            let session = match Session::prepare(&fixture, workspace, config) {
                Ok(session) => session,
                Err(error) => {
                    self.add_evidence("准备失败", &format!("{error:#}"));
                    return;
                }
            };
            clear_list(&self.evidence_list);
            self.add_evidence(
                "原始件已冻结",
                &format!(
                    "只读副本 · SHA-256 {} · 工作区 {}",
                    session.manifest.source_video_sha256,
                    session.paths.root.display()
                ),
            );
            self.state.borrow_mut().session = Some(session);
        }
        let workspace = self
            .state
            .borrow()
            .session
            .as_ref()
            .map(|session| session.paths.root.clone());
        let Some(workspace) = workspace else {
            return;
        };
        let pause_flag = self.state.borrow().pause_flag.clone();
        pause_flag.store(false, Ordering::SeqCst);
        let (sender, receiver) = mpsc::channel();
        let progress_sender = sender.clone();
        let progress = Arc::new(move |event: ProgressEvent| {
            let _ = progress_sender.send(WorkerMsg::Progress(event));
        });
        let options = RunOptions {
            pause_requested: pause_flag,
            memory_budget_override: Some(budget),
            pause_before_frame_override: if pause_at_acceptance {
                pause_override.or(Some(Some(TARGET_FRAME)))
            } else {
                pause_override
            },
            progress: Some(progress),
        };
        thread::spawn(move || {
            let result = Session::open(&workspace)
                .and_then(|mut session| session.run(&options))
                .map_err(|error| format!("{error:#}"));
            let _ = sender.send(WorkerMsg::Finished(result));
        });
        {
            let mut state = self.state.borrow_mut();
            state.running = true;
            state.receiver = Some(receiver);
        }
        self.state_label.set_text("处理中");
        self.detail_label.set_text(&format!(
            "GStreamer 管线已启动 · {} KiB 预算 · {} 帧/块",
            budget / 1024,
            chunk_frames
        ));
        self.start_button.set_sensitive(false);
        self.pause_button.set_sensitive(true);
        self.resume_button.set_sensitive(false);
        self.checkpoint_button.set_sensitive(false);
        self.low_button.set_sensitive(false);
        self.blocked_button.set_sensitive(false);
    }

    fn choose_fixture(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder()
            .title("选择含 camera.json 的相机夹具目录")
            .modal(true)
            .build();
        let ui = self.clone();
        dialog.select_folder(
            Some(&self.window),
            None::<&gtk::gio::Cancellable>,
            move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    ui.state.borrow_mut().fixture_dir = path;
                    ui.state.borrow_mut().session = None;
                    clear_list(&ui.evidence_list);
                    ui.acceptance_after.set_text("尚未生成");
                    ui.refresh_fixture();
                    ui.add_evidence(
                        "已选择夹具",
                        &ui.state.borrow().fixture_dir.display().to_string(),
                    );
                }
            },
        );
    }

    fn poll_worker(self: &Rc<Self>) {
        let messages = {
            let state = self.state.borrow();
            let mut messages = Vec::new();
            if let Some(receiver) = state.receiver.as_ref() {
                loop {
                    match receiver.try_recv() {
                        Ok(message) => messages.push(message),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => break,
                    }
                }
            }
            messages
        };
        for message in messages {
            match message {
                WorkerMsg::Progress(event) => self.handle_progress(event),
                WorkerMsg::Finished(result) => self.finish_worker(result),
            }
        }
    }

    fn finish_worker(self: &Rc<Self>, result: Result<RunOutcome, String>) {
        {
            let mut state = self.state.borrow_mut();
            state.running = false;
            state.receiver = None;
        }
        match result {
            Ok(RunOutcome::Completed { report, .. }) => {
                self.add_evidence(
                    "修复完成",
                    &format!(
                        "输出 SHA-256 {} · 验收 {} 项",
                        report.output_metadata_sha256,
                        report.checks.len()
                    ),
                );
            }
            Ok(RunOutcome::Paused { checkpoint }) => {
                self.add_evidence(
                    "安全暂停",
                    &format!(
                        "帧 {} · pts {} ns · video 0x{:X} · metadata 0x{:X}",
                        checkpoint.next_frame,
                        checkpoint.next_pts_ns,
                        checkpoint.next_video_offset,
                        checkpoint.next_metadata_offset
                    ),
                );
            }
            Ok(RunOutcome::ResourcePaused { checkpoint, plan }) => {
                self.add_evidence(
                    "资源不足，已保留检查点",
                    &format!(
                        "帧 {} · 需要 {} 字节 / 预算 {} 字节 · {}",
                        checkpoint.next_frame,
                        plan.estimated_bytes,
                        plan.budget_bytes,
                        plan.degraded_reason.unwrap_or_default()
                    ),
                );
            }
            Err(error) => {
                self.add_evidence("处理失败（证据已保留）", &error);
            }
        }
        let workspace = self
            .state
            .borrow()
            .session
            .as_ref()
            .map(|session| session.paths.root.clone());
        if let Some(workspace) = workspace
            && let Ok(session) = Session::open(&workspace)
        {
            self.state.borrow_mut().session = Some(session);
        }
        self.refresh_fixture();
        self.refresh_acceptance_report();
    }

    fn handle_progress(&self, event: ProgressEvent) {
        match event {
            ProgressEvent::Started {
                first_frame,
                total_frames,
                resumed,
            } => {
                self.state_label.set_text(if resumed {
                    "从检查点恢复"
                } else {
                    "处理中"
                });
                self.detail_label.set_text(&format!(
                    "起始帧 {first_frame} / 共 {total_frames} 帧；GStreamer 输出逐帧进入分析器。"
                ));
            }
            ProgressEvent::Frame {
                frame,
                pts_ns,
                video_offset,
                metadata_offset,
                before,
                after,
                preview_rgba,
                object_present,
            } => {
                let profile = self
                    .state
                    .borrow()
                    .session
                    .as_ref()
                    .map(|session| session.manifest.config.profile.clone());
                if let Some(profile) = profile.as_ref() {
                    self.progress
                        .set_fraction(f64::from(frame + 1) / f64::from(profile.frame_count.max(1)));
                    if let Some(rgba) = preview_rgba.as_ref() {
                        self.set_preview(rgba, profile);
                        self.preview_caption.set_text(&format!(
                            "frame {frame} · pts {:.6}s · offset 0x{video_offset:X} · object={}",
                            pts_ns as f64 / 1_000_000_000.0,
                            if object_present { "yes" } else { "no" }
                        ));
                    }
                }
                if frame == TARGET_FRAME {
                    self.acceptance_after.set_text(&describe_metadata(&after));
                    self.acceptance_state.set_text("修复结果已生成");
                    self.acceptance_state.remove_css_class("warn");
                    self.acceptance_state.add_css_class("accent");
                }
                if frame == TARGET_FRAME || after.has_repeated_object() {
                    self.add_evidence(
                        &format!(
                            "帧 {frame} · {:.3}s · 重复对象 {}",
                            pts_ns as f64 / 1_000_000_000.0,
                            if object_present {
                                "命中"
                            } else {
                                "未命中"
                            }
                        ),
                        &format!(
                            "video 0x{video_offset:X} / metadata 0x{metadata_offset:X}\n\
                             BEFORE CLL {} FALL {} · AFTER CLL {} FALL {}",
                            before.max_cll, before.max_fall, after.max_cll, after.max_fall
                        ),
                    );
                }
                self.state_label.set_text("处理中");
                self.detail_label.set_text(&format!(
                    "帧 {frame} · pts {} ns · video 0x{:X} · meta 0x{:X}",
                    pts_ns, video_offset, metadata_offset
                ));
            }
            ProgressEvent::ChunkCommitted {
                next_frame,
                total_frames,
                plan,
            } => {
                self.progress
                    .set_fraction(f64::from(next_frame) / f64::from(total_frames.max(1)));
                self.detail_label.set_text(&format!(
                    "检查点已原子写入：下一帧 {next_frame} / {total_frames} · 块 {} 帧 · {}",
                    plan.chunk_frames,
                    plan.degraded_reason.unwrap_or_else(|| "全质量".into())
                ));
            }
            ProgressEvent::Paused {
                frame,
                pts_ns,
                video_offset,
                metadata_offset,
                reason,
            } => {
                self.state_label.set_text("已安全暂停");
                self.detail_label.set_text(&format!(
                    "{reason:?} · 帧 {frame} · pts {pts_ns} ns · video 0x{video_offset:X} · meta 0x{metadata_offset:X}"
                ));
            }
            ProgressEvent::ResourcePaused {
                frame,
                pts_ns,
                video_offset,
                metadata_offset,
                plan,
            } => {
                self.state_label.set_text("资源不足，安全暂停");
                self.detail_label.set_text(&format!(
                    "帧 {frame} · pts {pts_ns} ns · video 0x{video_offset:X} · meta 0x{metadata_offset:X}\n{}",
                    plan.degraded_reason.unwrap_or_default()
                ));
                self.add_evidence(
                    "资源预算不足",
                    &format!(
                        "需要 {} 字节，预算 {} 字节；检查点保留在帧 {frame} / 0x{video_offset:X}",
                        plan.estimated_bytes, plan.budget_bytes
                    ),
                );
            }
            ProgressEvent::Completed {
                acceptance_path,
                output_sha256,
            } => {
                self.state_label.set_text("修复完成");
                self.detail_label
                    .set_text(&format!("输出 SHA-256 {output_sha256}"));
                self.add_evidence("验收文件已生成", &acceptance_path.display().to_string());
            }
        }
    }

    fn refresh_acceptance_report(&self) {
        let acceptance_path = self
            .state
            .borrow()
            .session
            .as_ref()
            .map(|session| session.paths.acceptance.clone());
        let Some(path) = acceptance_path else {
            return;
        };
        let Ok(data) = fs::read(&path) else {
            return;
        };
        let Ok(report) = serde_json::from_slice::<AcceptanceReport>(&data) else {
            return;
        };
        let Some(check) = report
            .checks
            .iter()
            .find(|check| check.frame == TARGET_FRAME)
        else {
            return;
        };
        self.acceptance_time
            .set_text(&format!("{:.9} s", check.pts_ns as f64 / 1_000_000_000.0));
        self.acceptance_frame
            .set_text(&format!("帧 {} / pts {} ns", check.frame, check.pts_ns));
        self.acceptance_video_offset.set_text(&format!(
            "视频原始偏移 {} ({})",
            check.video_offset_hex, check.video_offset
        ));
        self.acceptance_meta_offset.set_text(&format!(
            "元数据原始偏移 {} ({})",
            check.metadata_offset_hex, check.metadata_offset
        ));
        self.acceptance_before
            .set_text(&describe_metadata(&check.before));
        self.acceptance_after
            .set_text(&describe_metadata(&check.after));
        if check.pass {
            self.acceptance_state.set_text("PASS · 时间与原始偏移一致");
            self.acceptance_state.remove_css_class("warn");
            self.acceptance_state.add_css_class("accent");
        } else {
            self.acceptance_state
                .set_text("FAIL · 检查 acceptance.json");
            self.acceptance_state.remove_css_class("accent");
            self.acceptance_state.add_css_class("danger");
        }
        let resume = report
            .resume_comparison
            .as_ref()
            .map(|comparison| {
                format!(
                    "pause@{} prefix={} resumed@{}",
                    comparison.paused_frame,
                    comparison.prefix_verified,
                    comparison.resumed_from_frame
                )
            })
            .unwrap_or_else(|| "无暂停恢复".into());
        self.add_evidence(
            "验收点 PASS",
            &format!(
                "{} · {} → {}\nvideo {} / metadata {} · {}",
                check.pts_ns,
                check.before.max_cll,
                check.after.max_cll,
                check.video_offset_hex,
                check.metadata_offset_hex,
                resume
            ),
        );
    }
}
