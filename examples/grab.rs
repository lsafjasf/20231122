use subtitle_safe_review::analysis::Analyzer;
use subtitle_safe_review::model::ExtremeParams;

fn main() {
    let uri = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "file:///tmp/ssr-test.ogg".into());
    let an = Analyzer::new(&uri).expect("analyzer");
    println!("duration = {}ms", an.duration_ms());
    let p = ExtremeParams::tightened();
    for ms in [0u64, 500, 960, 1000, 2000, 3900] {
        an.debug_seek(ms);
        match an.debug_pull(&p) {
            Ok(Some((pts, ink))) => println!("seek {ms} -> pts {pts}ms ink_ratio {ink:.4}"),
            Ok(None) => println!("seek {ms} -> NONE"),
            Err(e) => println!("seek {ms} -> ERR {e:#}"),
        }
    }
}
