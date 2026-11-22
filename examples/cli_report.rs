use std::path::PathBuf;
use syncwall::analyze::analyze;
use syncwall::model::Verdict;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths: Vec<PathBuf> = if args.is_empty() {
        vec![
            "artifacts/session-truncated.jsonl".into(),
            "artifacts/session-replay.jsonl".into(),
        ]
    } else {
        args.iter().map(PathBuf::from).collect()
    };
    let refs: Vec<&std::path::Path> = paths.iter().map(|p| p.as_path()).collect();
    let r = analyze(&refs).unwrap();
    println!("== 处理步骤 ==");
    for s in &r.steps {
        println!("  [{}] {} — {}", s.index, s.name, s.detail);
    }
    println!("== 结论 ==");
    for f in &r.findings {
        let v = match f.verdict {
            Verdict::Ok => "OK ",
            Verdict::Indeterminate => "???",
            Verdict::Error => "ERR",
        };
        println!(
            "  {} #{} [{}] ch{:?} @{:?}ms {}",
            v, f.id, f.kind, f.channel, f.at_ms, f.summary
        );
        for e in &f.evidence {
            println!("      证据 {}:{} — {}", e.source_file, e.line_no, e.note);
        }
    }
    println!("== 第一个错误状态 ==>");
    match r.first_error {
        Some(id) => {
            let f = &r.findings[id];
            println!("  #{} [{}] {}", f.id, f.kind, f.summary);
        }
        None => println!("  无"),
    }
    println!(
        "错误 {} 处 / 无法判断 {} 处",
        r.error_count, r.indeterminate_count
    );
}
