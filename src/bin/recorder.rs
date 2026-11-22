use std::path::PathBuf;
use syncwall::gst_src::{record, replay, RecordOptions};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // 用法: recorder <out.jsonl> [--frames N] [--channels N] [--drop a,b]
    //        [--truncate-at N] [--stall-at N --stall-ms M] [--replay-of ORIG]
    let out = PathBuf::from(args.get(1).map(|s| s.as_str()).unwrap_or("session.jsonl"));
    let mut opt = RecordOptions {
        channels: 3,
        frames: 30,
        tick_ms: 33,
        drop_seqs: vec![],
        truncate_at: None,
        stall_at: None,
        stall_ms: 120,
    };
    let mut replay_of: Option<PathBuf> = None;
    let mut i = 2;
    while i < args.len() {
        let val = |i: &mut usize| -> String {
            *i += 1;
            args[*i].clone()
        };
        match args[i].as_str() {
            "--frames" => opt.frames = val(&mut i).parse().unwrap(),
            "--channels" => opt.channels = val(&mut i).parse().unwrap(),
            "--tick-ms" => opt.tick_ms = val(&mut i).parse().unwrap(),
            "--drop" => {
                opt.drop_seqs = val(&mut i)
                    .split(',')
                    .map(|s| s.trim().parse().unwrap())
                    .collect()
            }
            "--truncate-at" => opt.truncate_at = Some(val(&mut i).parse().unwrap()),
            "--stall-at" => opt.stall_at = Some(val(&mut i).parse().unwrap()),
            "--stall-ms" => opt.stall_ms = val(&mut i).parse().unwrap(),
            "--replay-of" => replay_of = Some(PathBuf::from(val(&mut i))),
            other => eprintln!("未知参数: {}", other),
        }
        i += 1;
    }
    let r = match &replay_of {
        Some(orig) => replay(orig, &out, &opt),
        None => record(&out, &opt),
    };
    match r {
        Ok(()) => println!("已写入 {}", out.display()),
        Err(e) => {
            eprintln!("采集失败: {}", e);
            std::process::exit(1);
        }
    }
}
