//! Loop-seam analysis and repair.
//!
//! A "cut" picks a loop start `s` and loop end `e` (frame indices into the
//! mono signal); the loop body is `x[s..e]`. Candidates are scored on
//! waveform/phase continuity across the wrap and on per-band energy match
//! between the material just before `e` and just after `s`.

pub const NUM_BANDS: usize = 3;
pub const BAND_NAMES: [&str; NUM_BANDS] = ["low", "mid", "high"];

#[derive(Clone, Copy, Debug)]
pub struct Candidate {
    pub start: usize,
    pub end: usize,
    pub score: f64,
    /// |x[end-1] - x[start]| normalized by signal RMS.
    pub value_jump: f64,
    /// Slope mismatch across the wrap, normalized by signal RMS.
    pub slope_jump: f64,
    /// Sum of |log2(end_energy/start_energy)| across bands.
    pub band_mismatch: f64,
}

impl Candidate {
    pub fn loop_len(&self) -> usize {
        self.end - self.start
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SeamMetrics {
    /// max |x[i] - x[i-1]| in a small window around the wrap point.
    pub discontinuity: f64,
    /// RMS of the first `window` frames of the loop (dBFS).
    pub head_rms_db: f64,
    /// RMS of the last `window` frames of the loop (dBFS).
    pub tail_rms_db: f64,
    /// head_rms_db - tail_rms_db
    pub energy_diff_db: f64,
    /// Per-band energy ratio (tail/head) in dB.
    pub band_diff_db: [f64; NUM_BANDS],
}

/// RBJ biquad, Direct Form I.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 3],
}

impl Biquad {
    fn lowpass(sr: f64, f0: f64) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * f0 / sr;
        let alpha = w0.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let c = w0.cos();
        let a0 = 1.0 + alpha;
        Self {
            b: [(1.0 - c) / 2.0 / a0, (1.0 - c) / a0, (1.0 - c) / 2.0 / a0],
            a: [1.0, -2.0 * c / a0, (1.0 - alpha) / a0],
        }
    }

    fn highpass(sr: f64, f0: f64) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * f0 / sr;
        let alpha = w0.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let c = w0.cos();
        let a0 = 1.0 + alpha;
        Self {
            b: [(1.0 + c) / 2.0 / a0, -(1.0 + c) / a0, (1.0 + c) / 2.0 / a0],
            a: [1.0, -2.0 * c / a0, (1.0 - alpha) / a0],
        }
    }

    fn run(&self, x: &[f32]) -> Vec<f64> {
        let mut y = vec![0.0; x.len()];
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        for (i, &xi) in x.iter().enumerate() {
            let x0 = xi as f64;
            let y0 =
                self.b[0] * x0 + self.b[1] * x1 + self.b[2] * x2 - self.a[1] * y1 - self.a[2] * y2;
            y[i] = y0;
            x2 = x1;
            x1 = x0;
            y2 = y1;
            y1 = y0;
        }
        y
    }
}

/// Split `x` into low / mid / high band signals.
pub fn split_bands(x: &[f32], sr: u32) -> [Vec<f64>; NUM_BANDS] {
    let sr = sr as f64;
    let low = Biquad::lowpass(sr, 250.0).run(x);
    let high = Biquad::highpass(sr, 2500.0).run(x);
    let mid: Vec<f64> = x
        .iter()
        .zip(&low)
        .zip(&high)
        .map(|((&s, &l), &h)| s as f64 - l - h)
        .collect();
    [low, mid, high]
}

fn prefix_sums(x: &[f64]) -> Vec<f64> {
    let mut p = Vec::with_capacity(x.len() + 1);
    p.push(0.0);
    let mut acc = 0.0;
    for &v in x {
        acc += v * v;
        p.push(acc);
    }
    p
}

fn band_energies_at(
    band_prefix: &[Vec<f64>; NUM_BANDS],
    pos: usize,
    win: usize,
    before: bool,
) -> [f64; NUM_BANDS] {
    let mut out = [0.0; NUM_BANDS];
    for (b, p) in band_prefix.iter().enumerate() {
        let (lo, hi) = if before {
            (pos.saturating_sub(win), pos)
        } else {
            (pos, (pos + win).min(p.len() - 1))
        };
        out[b] = ((p[hi] - p[lo]) / (hi - lo).max(1) as f64).max(1e-12);
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FadeCurve {
    EqualPower,
    Linear,
    SCurve,
}

impl FadeCurve {
    pub const ALL: [FadeCurve; 3] = [FadeCurve::EqualPower, FadeCurve::Linear, FadeCurve::SCurve];

    pub fn label(&self) -> &'static str {
        match self {
            FadeCurve::EqualPower => "等功率",
            FadeCurve::Linear => "线性",
            FadeCurve::SCurve => "S 曲线",
        }
    }

    /// (out_gain, in_gain) at t in [0, 1].
    fn gains(&self, t: f64) -> (f64, f64) {
        match self {
            FadeCurve::EqualPower => {
                let a = t * std::f64::consts::FRAC_PI_2;
                (a.cos(), a.sin())
            }
            FadeCurve::Linear => (1.0 - t, t),
            FadeCurve::SCurve => {
                let s = t * t * (3.0 - 2.0 * t);
                (1.0 - s, s)
            }
        }
    }
}

/// Build the repaired loop (mono) for a candidate cut.
///
/// The crossfade overlaps the last `fade` frames before `end` with the first
/// `fade` frames after `start`; the fade consumes `fade` frames, so the
/// resulting loop is `end - start - fade` frames long and its periodic
/// extension is continuous by construction.
pub fn build_loop(mono: &[f32], c: &Candidate, fade: usize, curve: FadeCurve) -> Vec<f32> {
    let body = c.end - c.start;
    let fade = fade.min(body / 4);
    if fade == 0 {
        return mono[c.start..c.end].to_vec();
    }
    let mut out = Vec::with_capacity(body - fade);
    for i in 0..fade {
        let t = (i as f64 + 0.5) / fade as f64;
        let (g_out, g_in) = curve.gains(t);
        let tail = mono[c.end - fade + i] as f64;
        let head = mono[c.start + i] as f64;
        out.push((g_out * tail + g_in * head) as f32);
    }
    out.extend_from_slice(&mono[c.start + fade..c.end - fade]);
    out
}

/// Naive direct join (no cut optimisation, no fade) for A/B comparison.
pub fn naive_join(mono: &[f32]) -> Vec<f32> {
    mono.to_vec()
}

/// Metrics describing the wrap seam of a finished loop.
pub fn seam_metrics(loop_samples: &[f32], sr: u32) -> SeamMetrics {
    let n = loop_samples.len();
    if n < 8 {
        return SeamMetrics::default();
    }
    // Discontinuity: largest sample-to-sample step across the wrap,
    // probing two frames either side of the n-1 -> 0 junction.
    let mut disc = 0.0f64;
    for k in 0..4usize {
        let i = (n + k - 2) % n;
        let j = (i + 1) % n;
        disc = disc.max((loop_samples[j] as f64 - loop_samples[i] as f64).abs());
    }
    let win = (0.05 * sr as f64) as usize;
    let head = rms(&loop_samples[..win.min(n)]);
    let tail = rms(&loop_samples[n - win.min(n)..]);
    let head_db = 20.0 * head.log10();
    let tail_db = 20.0 * tail.log10();

    let bands = split_bands(loop_samples, sr);
    let mut band_diff = [0.0; NUM_BANDS];
    for (b, sig) in bands.iter().enumerate() {
        let p = prefix_sums(sig);
        let e_head = ((p[win.min(n)] - p[0]) / win.min(n) as f64).max(1e-12);
        let e_tail = ((p[n] - p[n - win.min(n)]) / win.min(n) as f64).max(1e-12);
        band_diff[b] = 10.0 * (e_head / e_tail).log10();
    }
    SeamMetrics {
        discontinuity: disc,
        head_rms_db: head_db,
        tail_rms_db: tail_db,
        energy_diff_db: head_db - tail_db,
        band_diff_db: band_diff,
    }
}

/// In-place iterative radix-2 FFT. `re`/`im` must have the same
/// power-of-two length. Forward transform with `inverse = false`.
fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = 2.0 * std::f64::consts::PI / len as f64 * if inverse { 1.0 } else { -1.0 };
        let (wr, wi) = (ang.cos(), ang.sin());
        let half = len / 2;
        for block in re.chunks_mut(len).zip(im.chunks_mut(len)) {
            let (bre, bim) = block;
            let (mut cwr, mut cwi) = (1.0f64, 0.0f64);
            for k in 0..half {
                let ur = bre[k];
                let ui = bim[k];
                let vr = bre[k + half] * cwr - bim[k + half] * cwi;
                let vi = bre[k + half] * cwi + bim[k + half] * cwr;
                bre[k] = ur + vr;
                bim[k] = ui + vi;
                bre[k + half] = ur - vr;
                bim[k + half] = ui - vi;
                (cwr, cwi) = (cwr * wr - cwi * wi, cwr * wi + cwi * wr);
            }
        }
        len <<= 1;
    }
    if inverse {
        let s = 1.0 / n as f64;
        for v in re.iter_mut() {
            *v *= s;
        }
        for v in im.iter_mut() {
            *v *= s;
        }
    }
}

/// Autocorrelation via FFT, valid for lags < x.len().
fn autocorrelate(x: &[f32]) -> Vec<f64> {
    let mut n = 1usize;
    while n < x.len() * 2 {
        n <<= 1;
    }
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    for (i, &v) in x.iter().enumerate() {
        re[i] = v as f64;
    }
    fft(&mut re, &mut im, false);
    for i in 0..n {
        let p = re[i] * re[i] + im[i] * im[i];
        re[i] = p;
        im[i] = 0.0;
    }
    fft(&mut re, &mut im, true);
    re.truncate(x.len());
    re
}

/// Dominant period of the material in frames, via autocorrelation.
/// Returns None for noise-like content.
pub fn estimate_period(mono: &[f32], sr: u32) -> Option<usize> {
    let n = mono.len().min((4.0 * sr as f64) as usize);
    let x = &mono[..n];
    let min_lag = (0.02 * sr as f64) as usize; // 20 ms
    let max_lag = (n / 2).min((2.0 * sr as f64) as usize);
    if max_lag <= min_lag {
        return None;
    }
    let ac = autocorrelate(x);
    let energy = ac[0].max(1e-12);
    let mut best_lag = 0;
    let mut best_corr = 0.30; // require a real correlation peak
    for (lag, &v) in ac.iter().enumerate().take(max_lag).skip(min_lag) {
        let corr = v / energy;
        if corr > best_corr {
            best_corr = corr;
            best_lag = lag;
        }
    }
    (best_lag > 0).then_some(best_lag)
}

/// Sample drift accumulated by continuous playback.
///
/// `period` is the material's natural cycle (e.g. one bar). If the loop
/// length is not an integer multiple of the period, every iteration shifts
/// the content by `residual` frames; `cumulative(iters)` projects the drift.
#[derive(Clone, Copy, Debug)]
pub struct Drift {
    pub period_frames: Option<usize>,
    pub residual_per_loop: i64,
}

impl Drift {
    pub fn cumulative(&self, iters: u64) -> i64 {
        self.residual_per_loop * iters as i64
    }
}

pub fn compute_drift(loop_len: usize, period: Option<usize>) -> Drift {
    let residual = match period {
        Some(p) if p > 0 => {
            let k = (loop_len as f64 / p as f64).round() as i64;
            loop_len as i64 - k * p as i64
        }
        _ => 0,
    };
    Drift {
        period_frames: period,
        residual_per_loop: residual,
    }
}

/// Apply the same cut/fade to every channel of an interleaved signal and
/// re-interleave, preserving the original channel count.
pub fn build_loop_multichannel(
    samples: &[f32],
    channels: u16,
    c: &Candidate,
    fade: usize,
    curve: FadeCurve,
) -> Vec<f32> {
    let ch = channels as usize;
    if ch == 1 {
        return build_loop(samples, c, fade, curve);
    }
    let frames = samples.len() / ch;
    let mut per_channel: Vec<Vec<f32>> = (0..ch)
        .map(|k| (0..frames).map(|i| samples[i * ch + k]).collect())
        .collect();
    for sig in &mut per_channel {
        *sig = build_loop(sig, c, fade, curve);
    }
    let out_frames = per_channel[0].len();
    let mut out = Vec::with_capacity(out_frames * ch);
    for i in 0..out_frames {
        for sig in &per_channel {
            out.push(sig[i]);
        }
    }
    out
}

fn rms(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 1.0;
    }
    (x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len() as f64)
        .sqrt()
        .max(1e-9)
}

/// Search loop-start / loop-end pairs near the file edges and return the
/// best `keep` candidates sorted by ascending score (lower is smoother).
///
/// `search_ms` bounds how far from each edge the cut points may move.
pub fn find_candidates(mono: &[f32], sr: u32, search_ms: f64, keep: usize) -> Vec<Candidate> {
    let n = mono.len();
    let w = ((search_ms * 0.001 * sr as f64) as usize)
        .min(n / 4)
        .max(64);
    let win = (0.032 * sr as f64) as usize; // band-energy window
    let level = rms(mono);

    let bands = split_bands(mono, sr);
    let band_prefix: [Vec<f64>; NUM_BANDS] = [0, 1, 2].map(|b| prefix_sums(&bands[b]));

    // Per-point features for candidate starts (near head) and ends (near tail).
    let starts: Vec<usize> = (0..w).collect();
    let ends: Vec<usize> = (n - w..n).collect();
    let feat = |pos: usize, before: bool| {
        let e = band_energies_at(&band_prefix, pos, win, before);
        [e[0].log2(), e[1].log2(), e[2].log2()]
    };
    let start_feat: Vec<_> = starts.iter().map(|&s| feat(s, false)).collect();
    let end_feat: Vec<_> = ends.iter().map(|&e| feat(e, true)).collect();

    let min_len = n / 2;
    let score_pair = |s: usize, e: usize| -> Candidate {
        let si = s;
        let ei = e - (n - w);
        let sv = mono[s] as f64;
        let sslope = sv - mono[s.saturating_sub(1)] as f64;
        let ev = mono[e - 1] as f64;
        let eslope = ev - mono[e - 2] as f64;
        let value_jump = (ev - sv).abs() / level;
        let slope_jump = (eslope - sslope).abs() / level;
        let mut band_mismatch = 0.0;
        for b in 0..NUM_BANDS {
            band_mismatch += (end_feat[ei][b] - start_feat[si][b]).abs();
        }
        // Low band gets extra weight: sub-bass jumps are the most
        // audible artifact at a loop seam.
        band_mismatch += (end_feat[ei][0] - start_feat[si][0]).abs();
        // Prefer cuts that keep the loop close to the file length so
        // the musical grid (and thus playback drift) is preserved.
        let length_penalty = (n - (e - s)) as f64 / w as f64;
        let score = value_jump + 0.5 * slope_jump + 0.35 * band_mismatch + 0.15 * length_penalty;
        Candidate {
            start: s,
            end: e,
            score,
            value_jump,
            slope_jump,
            band_mismatch,
        }
    };

    // Coarse pass on an 8-frame grid, then refine the best pairs at
    // single-frame resolution.
    let coarse = 8usize;
    let mut rough: Vec<Candidate> = Vec::new();
    for &s in starts.iter().step_by(coarse) {
        for &e in ends.iter().step_by(coarse) {
            if e <= s || e - s < min_len {
                continue;
            }
            rough.push(score_pair(s, e));
        }
    }
    rough.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());
    let min_gap = (0.010 * sr as f64) as usize;
    rough = dedupe(rough, min_gap, 64);
    let mut best: Vec<Candidate> = Vec::new();
    for c in rough {
        let s_lo = c.start.saturating_sub(coarse);
        let s_hi = (c.start + coarse).min(w - 1);
        let e_lo = c.end.saturating_sub(coarse).max(n - w);
        let e_hi = (c.end + coarse).min(n - 1);
        for s in s_lo..=s_hi {
            for e in e_lo..=e_hi {
                if e <= s || e - s < min_len {
                    continue;
                }
                best.push(score_pair(s, e));
            }
        }
    }
    best.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());
    dedupe(best, min_gap, keep)
}

/// Keep at most one candidate per `min_gap`-frame neighbourhood.
fn dedupe(cands: Vec<Candidate>, min_gap: usize, keep: usize) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for c in cands {
        if out
            .iter()
            .any(|o| o.start.abs_diff(c.start) < min_gap && o.end.abs_diff(c.end) < min_gap)
        {
            continue;
        }
        out.push(c);
        if out.len() >= keep {
            break;
        }
    }
    out
}
