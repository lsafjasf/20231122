#![allow(clippy::chunks_exact_to_as_chunks)]

use anyhow::{anyhow, Context, Result};
use cairo::{Context as CairoContext, Format, ImageSurface};
use credits_core::model::TypesetParameters;
use credits_core::report::PixelDiff;
use pangocairo::pango;
use pangocairo::pango::{Alignment, FontDescription, WrapMode};

pub fn render_overlay(
    text: &str,
    width: u32,
    height: u32,
    parameters: &TypesetParameters,
    elapsed_s: f64,
) -> Result<Vec<u8>> {
    let width_px = width.max(1) as i32;
    let height_px = height.max(1) as i32;
    let mut surface = ImageSurface::create(Format::ARgb32, width_px, height_px)
        .context("cannot create cairo overlay surface")?;
    {
        let cr = CairoContext::new(&surface).context("cannot create cairo overlay context")?;
        cr.set_operator(cairo::Operator::Source);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
        cr.paint().context("cannot clear cairo overlay surface")?;
        cr.set_operator(cairo::Operator::Over);

        let margin = (width as f64 * parameters.horizontal_margin).max(0.0);
        let layout = pangocairo::functions::create_layout(&cr);
        let description = FontDescription::from_string(&format!(
            "{} {}",
            parameters.font_family, parameters.font_size
        ));
        layout.set_font_description(Some(&description));
        layout.set_width(((width as f64 - margin * 2.0).max(8.0) * pango::SCALE as f64) as i32);
        layout.set_alignment(Alignment::Center);
        layout.set_wrap(WrapMode::WordChar);
        layout.set_spacing(
            ((parameters.line_spacing.max(1.0) - 1.0) * parameters.font_size * pango::SCALE as f64)
                as i32,
        );
        layout.set_text(text);

        let (_, text_height) = layout.pixel_size();
        let speed = parameters.scroll_speed_px_s.max(0.0);
        let cycle = if speed > 0.0 {
            (height as f64 + text_height as f64 + margin * 2.0) / speed
        } else {
            f64::INFINITY
        };
        let elapsed = (elapsed_s - parameters.roll_start_s).max(0.0);
        let phase = if cycle.is_finite() && cycle > 0.0 {
            elapsed % cycle
        } else {
            elapsed
        };
        let y = height as f64 + margin - phase * speed;
        let (red, green, blue, alpha) = parse_rgba(&parameters.color);

        if parameters.shadow {
            cr.move_to(margin + 2.0, y + 2.0);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.72 * parameters.opacity);
            pangocairo::functions::show_layout(&cr, &layout);
        }
        cr.move_to(margin, y);
        cr.set_source_rgba(red, green, blue, alpha * parameters.opacity);
        pangocairo::functions::show_layout(&cr, &layout);
    }

    surface.flush();
    let stride = surface.stride() as usize;
    let data = surface.data().map_err(|error| anyhow!("{error}"))?;
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for row in 0..height as usize {
        let start = row * stride;
        for pixel in data[start..start + width as usize * 4].chunks_exact(4) {
            let blue = pixel[0];
            let green = pixel[1];
            let red = pixel[2];
            let alpha = pixel[3];
            if alpha == 0 {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let alpha_factor = 255.0 / alpha as f64;
                rgba.push((red as f64 * alpha_factor).min(255.0) as u8);
                rgba.push((green as f64 * alpha_factor).min(255.0) as u8);
                rgba.push((blue as f64 * alpha_factor).min(255.0) as u8);
                rgba.push(alpha);
            }
        }
    }
    Ok(rgba)
}

pub fn blend_overlay(base: &[u8], overlay: &[u8]) -> Result<Vec<u8>> {
    if base.len() != overlay.len() || !base.len().is_multiple_of(4) {
        return Err(anyhow!(
            "RGBA buffers have incompatible sizes: {} and {}",
            base.len(),
            overlay.len()
        ));
    }
    let mut output = Vec::with_capacity(base.len());
    for (base_pixel, overlay_pixel) in base.chunks_exact(4).zip(overlay.chunks_exact(4)) {
        let alpha = overlay_pixel[3] as f64 / 255.0;
        let inverse = 1.0 - alpha;
        for channel in 0..3 {
            let value =
                overlay_pixel[channel] as f64 * alpha + base_pixel[channel] as f64 * inverse;
            output.push(value.round().clamp(0.0, 255.0) as u8);
        }
        output.push(255);
    }
    Ok(output)
}

pub fn pixel_diff(before: &[u8], after: &[u8]) -> Result<PixelDiff> {
    if before.len() != after.len() || !before.len().is_multiple_of(4) {
        return Err(anyhow!("cannot compare differently sized RGBA buffers"));
    }
    let mut sum = 0_u64;
    let mut changed = 0_u64;
    let pixels = (before.len() / 4) as u64;
    for (left, right) in before.chunks_exact(4).zip(after.chunks_exact(4)) {
        let mut pixel_delta = 0_u32;
        for channel in 0..3 {
            pixel_delta += left[channel].abs_diff(right[channel]) as u32;
        }
        sum += pixel_delta as u64;
        if pixel_delta > 8 {
            changed += 1;
        }
    }
    let denominator = (pixels.max(1) * 3) as f64;
    Ok(PixelDiff {
        mean_abs: sum as f64 / denominator,
        changed_ratio: changed as f64 / pixels.max(1) as f64,
        compared_pixels: pixels,
    })
}

fn parse_rgba(raw: &str) -> (f64, f64, f64, f64) {
    let value = raw.trim().trim_start_matches('#');
    let expanded = match value.len() {
        3 => value
            .chars()
            .flat_map(|character| [character, character])
            .collect::<String>(),
        6 => format!("{value}FF"),
        _ => value.to_string(),
    };
    if expanded.len() != 8 {
        return (1.0, 1.0, 1.0, 1.0);
    }
    let parse = |offset: usize| {
        u8::from_str_radix(&expanded[offset..offset + 2], 16)
            .map(|value| value as f64 / 255.0)
            .unwrap_or(1.0)
    };
    (parse(0), parse(2), parse(4), parse(6))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blends_opaque_overlay() {
        let base = vec![0, 0, 0, 255, 10, 20, 30, 255];
        let overlay = vec![255, 255, 255, 255, 0, 0, 0, 0];
        let result = blend_overlay(&base, &overlay).unwrap();
        assert_eq!(&result[..4], &[255, 255, 255, 255]);
        assert_eq!(&result[4..8], &[10, 20, 30, 255]);
    }

    #[test]
    fn computes_pixel_difference() {
        let before = vec![0, 0, 0, 255];
        let after = vec![255, 0, 0, 255];
        let diff = pixel_diff(&before, &after).unwrap();
        assert!((diff.mean_abs - 85.0).abs() < 0.001);
        assert_eq!(diff.changed_ratio, 1.0);
    }
}
