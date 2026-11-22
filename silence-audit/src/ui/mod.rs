//! GTK4 复核界面：波形绘制、静音段高亮与离屏 PNG 证据导出。

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use crate::gtk::gdk_pixbuf::Pixbuf;
use crate::gtk::glib;
use crate::gtk::prelude::*;
use crate::gtk::{cairo, ApplicationWindow, DrawingArea, Label, ListBox, Paned, ScrolledWindow};

use crate::model::{ObjectResult, SilenceSegment, Waveform};

/// 绘制所需的全部视图数据。
#[derive(Clone)]
struct ViewData {
    object_id: String,
    waveform: Waveform,
    segments: Vec<SilenceSegment>,
    status_text: String,
}

/// 把单个对象结果渲染为 PNG 证据（无需可见窗口，使用 cairo image surface）。
pub fn render_png(
    result: &ObjectResult,
    out_path: &Path,
    width: i32,
    height: i32,
) -> anyhow::Result<()> {
    let waveform = result
        .waveform
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("对象 {} 缺少波形数据，无法渲染", result.object_id))?;
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let cr = cairo::Context::new(&surface)?;
    draw_waveform(
        &cr,
        width,
        height,
        waveform,
        &result.segments,
        &result.object_id,
    )?;
    drop(cr);
    surface.flush();

    let pixbuf = pixbuf_from_surface(surface, width, height)?;
    pixbuf.savev(out_path, "png", &[])?;
    Ok(())
}

/// 交互式复核窗口：左侧对象列表，右侧波形与静音段明细。
pub fn run_review_window(app: &crate::gtk::Application, results: Vec<ObjectResult>) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("音轨静音段识别 - 复核")
        .default_width(1100)
        .default_height(680)
        .build();

    let views: Vec<ViewData> = results
        .into_iter()
        .filter_map(|r| {
            let waveform = r.waveform?;
            let segment_count = r.segments.len();
            let status = format!("{:?} · 静音段 {} 处", r.status, segment_count);
            Some(ViewData {
                object_id: r.object_id,
                waveform,
                segments: r.segments,
                status_text: status,
            })
        })
        .collect();
    let views = Rc::new(views);

    let paned = Paned::builder()
        .orientation(crate::gtk::Orientation::Horizontal)
        .build();
    let list = ListBox::new();
    let scrolled = ScrolledWindow::builder()
        .hscrollbar_policy(crate::gtk::PolicyType::Never)
        .min_content_width(240)
        .child(&list)
        .build();

    let drawing = DrawingArea::builder()
        .content_width(800)
        .content_height(360)
        .vexpand(true)
        .hexpand(true)
        .build();
    let detail = Label::builder()
        .label("选择左侧对象查看静音段明细")
        .margin_top(8)
        .margin_bottom(8)
        .wrap(true)
        .build();
    let right_box = crate::gtk::Box::new(crate::gtk::Orientation::Vertical, 6);
    right_box.append(&drawing);
    right_box.append(&detail);

    paned.set_start_child(Some(&scrolled));
    paned.set_end_child(Some(&right_box));
    paned.set_position(260);
    window.set_child(Some(&paned));

    let current: Rc<RefCell<Option<ViewData>>> = Rc::new(RefCell::new(views.first().cloned()));
    for view in views.iter() {
        let row = crate::gtk::Box::new(crate::gtk::Orientation::Vertical, 2);
        row.append(
            &Label::builder()
                .label(view.object_id.as_str())
                .xalign(0.0)
                .build(),
        );
        row.append(
            &Label::builder()
                .label(view.status_text.as_str())
                .xalign(0.0)
                .css_classes(["dim-label"])
                .build(),
        );
        list.append(&row);
    }

    let current_for_draw = Rc::clone(&current);
    drawing.set_draw_func(move |_area, cr, width, height| {
        if let Some(view) = current_for_draw.borrow().as_ref() {
            if let Err(err) = draw_waveform(
                cr,
                width,
                height,
                &view.waveform,
                &view.segments,
                &view.object_id,
            ) {
                eprintln!("波形绘制失败: {err}");
            }
        }
    });

    let current_for_select = Rc::clone(&current);
    let views_for_select = Rc::clone(&views);
    let drawing_for_select = drawing.clone();
    let detail_for_select = detail.clone();
    list.connect_row_selected(move |_list, row| {
        if let Some(row) = row {
            let idx = row.index() as usize;
            if let Some(view) = views_for_select.get(idx) {
                *current_for_select.borrow_mut() = Some(view.clone());
                detail_for_select.set_text(&segment_text(view));
                drawing_for_select.queue_draw();
            }
        }
    });

    if let Some(first) = views.first() {
        detail.set_text(&segment_text(first));
    }
    window.present();
}

fn segment_text(view: &ViewData) -> String {
    if view.segments.is_empty() {
        return format!("{}：未检测到静音段", view.object_id);
    }
    view.segments
        .iter()
        .map(|s| {
            let end = s
                .end_secs
                .map(|e| format!("{e:.3}s"))
                .unwrap_or_else(|| "开放(截断)".to_string());
            format!(
                "#{} {} [{:.3}s → {}] 帧 {}→{} RMS {:.1}dB 峰值 {:.1}dB",
                s.index,
                s.track,
                s.start_secs,
                end,
                s.start_frame,
                s.end_frame
                    .map(|f| f.to_string())
                    .unwrap_or_else(|| "开放".to_string()),
                s.min_rms_db,
                s.max_peak_db,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 通用波形绘制：背景、静音段红色半透明遮罩、RMS 包络与峰值折线。
fn draw_waveform(
    cr: &cairo::Context,
    width: i32,
    height: i32,
    waveform: &Waveform,
    segments: &[SilenceSegment],
    title: &str,
) -> anyhow::Result<()> {
    let w = width as f64;
    let h = height as f64;
    let margin = 48.0;
    let plot_w = (w - margin * 2.0).max(10.0);
    let plot_h = (h - margin * 2.0).max(10.0);
    let mid_y = margin + plot_h / 2.0;
    let duration = waveform.duration_secs.max(1e-9);

    cr.set_source_rgb(0.97, 0.97, 0.98);
    cr.paint()?;

    // 静音段遮罩。
    for seg in segments {
        let x0 = margin + seg.start_secs / duration * plot_w;
        let x1 = match seg.end_secs {
            Some(end) => margin + (end / duration * plot_w).min(plot_w + margin),
            None => margin + plot_w,
        };
        cr.set_source_rgba(0.90, 0.25, 0.25, if seg.truncated { 0.28 } else { 0.18 });
        cr.rectangle(x0, margin, (x1 - x0).max(1.0), plot_h);
        cr.fill()?;
    }

    // 中线与边框。
    cr.set_source_rgb(0.75, 0.75, 0.78);
    cr.move_to(margin, mid_y);
    cr.line_to(margin + plot_w, mid_y);
    cr.stroke()?;
    cr.set_source_rgb(0.55, 0.55, 0.60);
    cr.rectangle(margin, margin, plot_w, plot_h);
    cr.stroke()?;

    let n = waveform.peaks.len().max(1) as f64;
    // RMS 填充包络。
    cr.set_source_rgba(0.20, 0.45, 0.85, 0.35);
    cr.move_to(margin, mid_y);
    for (i, value) in waveform.rms.iter().enumerate() {
        let x = margin + i as f64 / n * plot_w;
        let amp = value.clamp(0.0, 1.0) as f64 * plot_h / 2.0;
        cr.line_to(x, mid_y - amp);
    }
    for (i, value) in waveform.rms.iter().enumerate().rev() {
        let x = margin + i as f64 / n * plot_w;
        let amp = value.clamp(0.0, 1.0) as f64 * plot_h / 2.0;
        cr.line_to(x, mid_y + amp);
    }
    cr.close_path();
    cr.fill()?;

    // 峰值折线。
    cr.set_source_rgb(0.10, 0.30, 0.70);
    cr.set_line_width(1.0);
    for (i, value) in waveform.peaks.iter().enumerate() {
        let x = margin + i as f64 / n * plot_w;
        let amp = value.clamp(0.0, 1.0) as f64 * plot_h / 2.0;
        cr.move_to(x, mid_y - amp);
        cr.line_to(x, mid_y + amp);
    }
    cr.stroke()?;

    // 标题与时间轴。
    cr.set_source_rgb(0.12, 0.12, 0.14);
    cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    cr.set_font_size(14.0);
    cr.move_to(margin, margin - 16.0);
    cr.show_text(&format!(
        "{title} · {:.3}s · {}Hz",
        duration, waveform.sample_rate
    ))?;
    cr.set_font_size(11.0);
    for tick in 0..=4 {
        let t = tick as f64 / 4.0;
        let x = margin + t * plot_w;
        cr.move_to(x, margin + plot_h + 16.0);
        cr.show_text(&format!("{:.2}s", t * duration))?;
    }
    Ok(())
}

/// cairo ARGB32 surface 转 gdk-pixbuf（预乘 alpha 转换后由 pixbuf 编码 PNG）。
fn pixbuf_from_surface(
    surface: cairo::ImageSurface,
    width: i32,
    height: i32,
) -> anyhow::Result<Pixbuf> {
    let stride = surface.stride() as usize;
    let data = surface
        .take_data()
        .map_err(|e| anyhow::anyhow!("读取 cairo surface 失败: {e:?}"))?;
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height as usize {
        let start = row * stride;
        for col in 0..width as usize {
            let base = start + col * 4;
            let (b, g, r, a) = (data[base], data[base + 1], data[base + 2], data[base + 3]);
            if a == 0 {
                pixels.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                pixels.extend_from_slice(&[
                    (r as u16 * 255 / a as u16) as u8,
                    (g as u16 * 255 / a as u16) as u8,
                    (b as u16 * 255 / a as u16) as u8,
                    a,
                ]);
            }
        }
    }
    let pixbuf = Pixbuf::from_mut_slice(
        pixels,
        crate::gtk::gdk_pixbuf::Colorspace::Rgb,
        true,
        8,
        width,
        height,
        width * 4,
    );
    Ok(pixbuf)
}

#[allow(dead_code)]
fn _glib_anchor() -> glib::Type {
    glib::Type::UNIT
}
