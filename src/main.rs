use std::path::Path;

use loop_joiner::dsp;
use loop_joiner::samples;
use loop_joiner::wav::{self, WavData};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--gensamples") => {
            let dir = args.get(2).map(String::as_str).unwrap_or("samples");
            gen_samples(Path::new(dir));
        }
        Some("--selftest") => selftest(),
        Some("--analyze") => {
            let path = args.get(2).expect("usage: loop-joiner --analyze <wav>");
            analyze_file(Path::new(path));
        }
        Some("--screenshot") => {
            let path = args.get(2).expect("usage: loop-joiner --screenshot <png>");
            loop_joiner::ui::run_screenshot(Path::new(path).to_path_buf());
        }
        _ => loop_joiner::ui::run(),
    }
}

fn gen_samples(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    for (name, wav) in samples::all() {
        let path = dir.join(format!("{name}.wav"));
        wav::write_wav(&path, &wav).unwrap();
        println!("wrote {} ({:.0} ms)", path.display(), wav.duration_ms());
    }
}

struct Report {
    name: String,
    naive: dsp::SeamMetrics,
    fixed: dsp::SeamMetrics,
    cut: dsp::Candidate,
    fade_frames: usize,
    drift: dsp::Drift,
    loop_len: usize,
    sr: u32,
}

fn process(name: &str, wav: &WavData) -> Report {
    let sr = wav.sample_rate;
    let mono = wav.mono();
    let naive = dsp::seam_metrics(&mono, sr);
    let candidates = dsp::find_candidates(&mono, sr, 120.0, 8);
    let cut = candidates[0];
    let fade_frames = (0.030 * sr as f64) as usize;
    let loop_samples = dsp::build_loop(&mono, &cut, fade_frames, dsp::FadeCurve::EqualPower);
    let fixed = dsp::seam_metrics(&loop_samples, sr);
    let period = dsp::estimate_period(&loop_samples, sr);
    let drift = dsp::compute_drift(loop_samples.len(), period);
    Report {
        name: name.to_string(),
        naive,
        fixed,
        cut,
        fade_frames,
        drift,
        loop_len: loop_samples.len(),
        sr,
    }
}

fn print_report(r: &Report) {
    let ms = |frames: usize| frames as f64 * 1000.0 / r.sr as f64;
    println!("== {}", r.name);
    println!(
        "  cut: start={} end={} (loop {:.0} ms, fade {:.1} ms)",
        r.cut.start,
        r.cut.end,
        ms(r.loop_len),
        ms(r.fade_frames)
    );
    println!(
        "  discontinuity: naive {:.4} -> fixed {:.4}",
        r.naive.discontinuity, r.fixed.discontinuity
    );
    println!(
        "  head/tail energy: naive {:+.2} dB -> fixed {:+.2} dB",
        r.naive.energy_diff_db, r.fixed.energy_diff_db
    );
    println!(
        "  band diff (L/M/H) fixed: {:+.2} {:+.2} {:+.2} dB",
        r.fixed.band_diff_db[0], r.fixed.band_diff_db[1], r.fixed.band_diff_db[2]
    );
    if let Some(p) = r.drift.period_frames {
        println!(
            "  drift: period {} frames, residual {:+}/loop, cumulative(100) {:+} frames ({:+.2} ms)",
            p,
            r.drift.residual_per_loop,
            r.drift.cumulative(100),
            r.drift.cumulative(100) as f64 * 1000.0 / r.sr as f64
        );
    } else {
        println!("  drift: no stable period (noise-like material)");
    }
}

fn selftest() {
    let mut ok = true;
    for (name, wav) in samples::all() {
        let r = process(name, &wav);
        print_report(&r);
        let click_gone =
            r.fixed.discontinuity < 0.05 && r.fixed.discontinuity < r.naive.discontinuity * 0.5;
        let energy_ok = r.fixed.energy_diff_db.abs() < r.naive.energy_diff_db.abs() + 0.5;
        println!(
            "  => click {} / energy {}",
            if click_gone { "OK" } else { "FAIL" },
            if energy_ok { "OK" } else { "FAIL" }
        );
        ok &= click_gone && energy_ok;
    }
    std::process::exit(if ok { 0 } else { 1 });
}

fn analyze_file(path: &Path) {
    let wav = wav::read_wav(path).unwrap();
    let r = process(&path.display().to_string(), &wav);
    print_report(&r);
}
