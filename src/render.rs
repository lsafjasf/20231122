use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use gtk4::cairo;
use gtk4::pango;

use crate::layout::CreditsLayout;
use crate::model::{BlockStyle, LayoutConfig};
use crate::pango_cairo;
use crate::samples;

#[link(name = "cairo")]
extern "C" {
    fn cairo_surface_write_to_png(
        surface: *mut cairo::ffi::cairo_surface_t,
        filename: *const std::os::raw::c_char,
    ) -> i32;
}

#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub data: Vec<u8>,
}

pub fn render_proof(
    layout: &CreditsLayout,
    time: f64,
    scale: f64,
    debug: bool,
) -> Result<Frame> {
    let width = (layout.canvas.width * scale).round().max(1.0) as i32;
    let height = (layout.canvas.height * scale).round().max(1.0) as i32;
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)
        .context("cannot allocate Cairo proof surface")?;
    {
        let context = cairo::Context::new(&surface)?;
        context.scale(scale, scale);
        draw_frame(&context, layout, time, debug)?;
        context.show_page()?;
    }
    surface.flush();
    let stride = surface.stride() as usize;
    let data = {
        let bytes = surface.data().context("cannot map Cairo proof surface")?;
        bytes.to_vec()
    };
    Ok(Frame {
        width: width as u32,
        height: height as u32,
        stride,
        data,
    })
}

pub fn draw_frame(
    context: &cairo::Context,
    layout: &CreditsLayout,
    time: f64,
    debug: bool,
) -> Result<()> {
    let canvas = layout.canvas;
    context.set_source_rgb(0.025, 0.031, 0.045);
    context.rectangle(0.0, 0.0, canvas.width, canvas.height);
    context.fill()?;

    let _ = context.save();
    draw_text_blocks(context, layout, time)?;
    let _ = context.restore();

    if debug {
        draw_guides(context, layout, time)?;
    } else {
        draw_pango_text(
            context,
            "CREDITS / SAFE-AREA MASTER",
            18.0,
            28.0,
            22.0,
            (0.96, 0.94, 0.88),
            0.55,
        );
    }
    Ok(())
}

fn draw_text_blocks(
    context: &cairo::Context,
    layout: &CreditsLayout,
    time: f64,
) -> Result<()> {
    let shift = layout.content_origin_y() - layout.scroll_offset(time);
    for block in &layout.blocks {
        let y = shift + block.y;
        if y > layout.canvas.height || y + block.height < 0.0 {
            continue;
        }
        let (red, green, blue, alpha) = text_color(block.style);
        context.set_source_rgba(red, green, blue, alpha);
        let _ = context.save();
        context.move_to(layout.safe_area.x, y);
        pango_cairo::show_layout(context, &block.layout);
        let _ = context.restore();
    }
    Ok(())
}

fn text_color(style: BlockStyle) -> (f64, f64, f64, f64) {
    match style {
        BlockStyle::Title => (1.0, 1.0, 1.0, 1.0),
        BlockStyle::Heading => (1.0, 0.98, 0.90, 0.96),
        BlockStyle::Body => (0.93, 0.95, 0.97, 0.92),
        BlockStyle::Caption => (0.70, 0.74, 0.79, 0.88),
    }
}

fn draw_guides(context: &cairo::Context, layout: &CreditsLayout, time: f64) -> Result<()> {
    let safe = layout.safe_area;
    context.set_source_rgba(0.15, 0.85, 0.48, 0.9);
    context.set_line_width(2.0);
    context.rectangle(safe.x, safe.y, safe.width, safe.height);
    context.stroke()?;
    draw_tiny_label(context, "SAFE AREA / VIEWPORT", safe.x + 8.0, safe.y - 8.0, (0.3, 1.0, 0.6));

    for line in &layout.lines {
        let rect = layout.screen_rect(line, time);
        if rect.bottom() < safe.y || rect.y > safe.bottom() {
            continue;
        }
        context.set_source_rgba(0.25, 0.62, 1.0, 0.9);
        context.set_line_width(1.1);
        context.rectangle(rect.x - 2.0, rect.y - 1.0, rect.width + 4.0, rect.height + 2.0);
        context.stroke()?;

        let baseline = layout.screen_baseline(line, time);
        context.set_source_rgba(1.0, 0.72, 0.18, 0.95);
        context.set_line_width(1.2);
        context.move_to(rect.x - 3.0, baseline);
        context.line_to(rect.x + rect.width + 3.0, baseline);
        context.stroke()?;

        draw_tiny_label(
            context,
            &line.line_id,
            10.0,
            rect.y + rect.height * 0.72,
            (0.62, 0.80, 1.0),
        );
    }

    let hud = format!(
        "{:.2}s  ·  {:.0} px/s  ·  {} visual lines  ·  content {:.0} px",
        time,
        layout.config.speed_px_per_second,
        layout.lines.len(),
        layout.content_height
    );
    draw_tiny_label(context, &hud, safe.x + 8.0, 52.0, (0.9, 0.9, 0.9));
    Ok(())
}

fn draw_tiny_label(
    context: &cairo::Context,
    text: &str,
    x: f64,
    y: f64,
    color: (f64, f64, f64),
) {
    let _ = context.save();
    draw_pango_text(context, text, 15.0, x, y - 13.0, color, 0.92);
    let _ = context.restore();
}

fn draw_pango_text(
    context: &cairo::Context,
    text: &str,
    size: f64,
    x: f64,
    y: f64,
    color: (f64, f64, f64),
    alpha: f64,
) {
    let pango_context = crate::pango_cairo::context_with_default_font_map();
    let text_layout = pango::Layout::new(&pango_context);
    let mut description = pango::FontDescription::new();
    description.set_family("DejaVu Sans");
    description.set_absolute_size(pango::units_from_double(size) as f64);
    text_layout.set_font_description(Some(&description));
    text_layout.set_text(text);
    context.set_source_rgba(color.0, color.1, color.2, alpha);
    context.move_to(x, y);
    pango_cairo::show_layout(context, &text_layout);
}

pub fn export_png(
    path: impl AsRef<Path>,
    layout: &CreditsLayout,
    time: f64,
    debug: bool,
) -> Result<PathBuf> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let frame = render_proof(layout, time, 1.0, debug)?;
    let mut surface = cairo::ImageSurface::create(
        cairo::Format::ARgb32,
        frame.width as i32,
        frame.height as i32,
    )?;
    {
        let mut bytes = surface.data()?;
        let contiguous = frame.stride == frame.width as usize * 4;
        if contiguous && bytes.len() == frame.data.len() {
            bytes.copy_from_slice(&frame.data);
        } else {
            for row in 0..frame.height as usize {
                let src = row * frame.stride;
                let dst = row * bytes.len() / frame.height as usize;
                let width = (frame.width * 4) as usize;
                bytes[dst..dst + width].copy_from_slice(&frame.data[src..src + width]);
            }
        }
    }
    let filename = CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("invalid proof path {}", path.display()))?;
    let status = unsafe {
        cairo_surface_write_to_png(surface.to_raw_none(), filename.as_ptr())
    };
    anyhow::ensure!(status == 0, "cairo PNG encoder returned status {status}");
    Ok(path.to_path_buf())
}

pub fn export_all_proofs(dir: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    let dir = dir.as_ref();
    let config = LayoutConfig::default();
    let mut outputs = Vec::new();
    for (sample, stem, fraction) in [
        (samples::mixed_sample(), "proof-mixed.png", 0.30),
        (samples::emoji_sample(), "proof-emoji.png", 0.24),
        (samples::long_line_sample(), "proof-long.png", 0.38),
    ] {
        let layout = crate::layout::build(&sample, &config)?;
        outputs.push(export_png(
            dir.join(stem),
            &layout,
            layout.duration * fraction,
            true,
        )?);
    }
    Ok(outputs)
}
