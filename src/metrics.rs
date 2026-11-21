//! 纯 Rust 帧指标计算：不依赖 GStreamer/GTK，便于单元测试。
use crate::model::ExtremeParams;

/// 一帧的墨迹统计
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InkStat {
    pub ink_pixels: u64,
    pub ink_ratio: f64,
    pub violated: bool,
}

fn luma(r: u8, g: u8, b: u8) -> u8 {
    // BT.601 近似
    ((66 * r as u32 + 129 * g as u32 + 25 * b as u32 + 128) >> 8) as u8
}

/// 统计 RGBA 帧中“安全区之外”的高亮（字幕状）像素。
/// 安全区由 ExtremeParams 的四向内缩比例定义。
pub fn analyze_frame_rgba(
    data: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    params: &ExtremeParams,
) -> InkStat {
    if width == 0 || height == 0 || data.len() < stride * height {
        return InkStat::default();
    }
    let x0 = (params.inset_left * width as f64) as usize;
    let x1 = width - (params.inset_right * width as f64) as usize;
    let y0 = (params.inset_top * height as f64) as usize;
    let y1 = height - (params.inset_bottom * height as f64) as usize;

    let mut outside_total: u64 = 0;
    let mut ink: u64 = 0;
    for y in 0..height {
        let row = &data[y * stride..y * stride + width * 4];
        let in_band = y < y0 || y >= y1;
        for x in 0..width {
            let inside_safe = !in_band && x >= x0 && x < x1;
            if inside_safe {
                continue;
            }
            outside_total += 1;
            let px = &row[x * 4..x * 4 + 4];
            if luma(px[0], px[1], px[2]) >= params.luma_threshold {
                ink += 1;
            }
        }
    }
    let ratio = if outside_total == 0 {
        0.0
    } else {
        ink as f64 / outside_total as f64
    };
    InkStat {
        ink_pixels: ink,
        ink_ratio: ratio,
        violated: ratio >= params.violation_ratio && ink > 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_frame(w: usize, h: usize, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut v = vec![0u8; w * h * 4];
        for px in v.chunks_exact_mut(4) {
            px.copy_from_slice(&[r, g, b, 255]);
        }
        v
    }

    #[test]
    fn dark_frame_has_no_ink() {
        let p = ExtremeParams::tightened();
        let f = solid_frame(64, 64, 10, 10, 10);
        let s = analyze_frame_rgba(&f, 64, 64, 64 * 4, &p);
        assert_eq!(s.ink_pixels, 0);
        assert!(!s.violated);
    }

    #[test]
    fn bright_subtitle_band_violates() {
        let p = ExtremeParams::tightened();
        let (w, h) = (100usize, 100usize);
        let mut f = solid_frame(w, h, 0, 0, 0);
        // 底部安全区外画一条白色“字幕”
        let y = 95;
        for x in 20..80 {
            let o = (y * w + x) * 4;
            f[o..o + 4].copy_from_slice(&[255, 255, 255, 255]);
        }
        let s = analyze_frame_rgba(&f, w, h, w * 4, &p);
        assert_eq!(s.ink_pixels, 60);
        assert!(s.ink_ratio > 0.0);
        assert!(s.violated);
    }

    #[test]
    fn bright_inside_safe_area_is_ignored() {
        let p = ExtremeParams::tightened();
        let (w, h) = (100usize, 100usize);
        let mut f = solid_frame(w, h, 0, 0, 0);
        let y = 50;
        for x in 40..60 {
            let o = (y * w + x) * 4;
            f[o..o + 4].copy_from_slice(&[255, 255, 255, 255]);
        }
        let s = analyze_frame_rgba(&f, w, h, w * 4, &p);
        assert_eq!(s.ink_pixels, 0);
    }

    #[test]
    fn loosened_params_less_sensitive() {
        let (w, h) = (100usize, 100usize);
        let mut f = solid_frame(w, h, 0, 0, 0);
        let y = 97;
        for x in 45..55 {
            let o = (y * w + x) * 4;
            f[o..o + 4].copy_from_slice(&[200, 200, 200, 255]);
        }
        let tight = analyze_frame_rgba(&f, w, h, w * 4, &ExtremeParams::tightened());
        let loose = analyze_frame_rgba(&f, w, h, w * 4, &ExtremeParams::loosened());
        assert!(tight.ink_pixels > loose.ink_pixels || loose.ink_pixels == 0);
    }
}
