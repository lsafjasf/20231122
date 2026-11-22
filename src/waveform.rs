//! 把输出波形（metric 序列）渲染成 RGB 图，作为可归档的证据 PNG。
pub const WAVE_W: u32 = 960;
pub const WAVE_H: u32 = 240;

/// 渲染波形：白底、蓝色波形、红色阈值线、第一个错误状态处画竖直红标记。
pub fn render(wave: &[f64], threshold: f64, first_error: Option<usize>) -> Vec<u8> {
    let (w, h) = (WAVE_W as usize, WAVE_H as usize);
    let mut img = vec![250u8; w * h * 3];
    let max_v = wave
        .iter()
        .cloned()
        .fold(threshold * 1.5, f64::max)
        .max(1e-9);
    let y_of = |v: f64| -> usize { h - 1 - ((v / max_v) * (h as f64 - 20.0) + 10.0) as usize };
    let mut set = |x: usize, y: usize, c: (u8, u8, u8)| {
        if x < w && y < h {
            let i = (y * w + x) * 3;
            img[i] = c.0;
            img[i + 1] = c.1;
            img[i + 2] = c.2;
        }
    };
    let ty = y_of(threshold);
    for x in 0..w {
        set(x, ty, (230, 60, 60));
    }
    if !wave.is_empty() {
        let n = wave.len();
        for x in 0..w {
            let idx = x * n / w;
            let y = y_of(wave[idx]);
            for dy in 0..2 {
                set(x, y + dy, (40, 90, 220));
            }
        }
    }
    if let Some(fe) = first_error {
        if !wave.is_empty() {
            let x = fe * (w - 1) / wave.len().max(1);
            for y in 0..h {
                set(x, y, (255, 0, 0));
                set(x + 1, y, (255, 0, 0));
            }
        }
    }
    img
}
