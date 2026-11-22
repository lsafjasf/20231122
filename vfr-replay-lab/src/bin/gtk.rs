//! GTK4 front-end: load a recorded journal, replay it, and render the result.
//!
//! Runs headlessly with the Broadway backend:
//!   GDK_BACKEND=broadway vfr-gtk <journal.jsonl> [snapshot.png]

use vfr_replay_lab::appui;
use vfr_replay_lab::persist::read_journal;
use vfr_replay_lab::replay::replay;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let journal = args
        .get(1)
        .expect("usage: vfr-gtk <journal.jsonl> [snapshot.png]");
    let snapshot = args.get(2).map(String::as_str);

    let entries = read_journal(journal).expect("read journal");
    let report = replay(&entries).expect("replay journal");
    println!(
        "loaded {} entries; replay_ok={}",
        entries.len(),
        report.is_ok()
    );

    let code = appui::run("lab.vfr.replay", &entries, report.is_ok(), snapshot);
    std::process::exit(code.get() as i32);
}
