fn main() {
    for (name, wav) in loop_joiner::samples::all() {
        let mono = wav.mono();
        let c = loop_joiner::dsp::find_candidates(&mono, wav.sample_rate, 120.0, 8);
        println!("{name}: {} candidates", c.len());
        for c in &c {
            println!("   s={} e={} score={:.3}", c.start, c.end, c.score);
        }
    }
}
