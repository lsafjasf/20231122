use anyhow::Result;

mod checkpoint;
mod compare;
mod engine;
mod evidence;
mod fixtures;
mod model;
mod ui;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "help".to_string());
    match cmd.as_str() {
        "fixtures" => fixtures::cmd(args.collect()),
        "run" => engine::cmd(args.collect()),
        "compare" => compare::cmd(args.collect()),
        "demo" => fixtures::cmd_demo(args.collect()),
        "ui" => ui::cmd(args.collect()),
        _ => {
            eprintln!(
                "plvcmp <command>\n  \
                 fixtures <dir>                 generate demo playlists + media\n  \
                 run --playlist P --out DIR [--pause-after N] [--resume]\n  \
                 compare --run-a DIR --run-b DIR --out FILE\n  \
                 demo [--root DIR]              full pause/resume/compare scenario\n  \
                 ui [--playlist P]              GTK4 front-end"
            );
            Ok(())
        }
    }
}
