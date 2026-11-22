mod experiment;
mod journal;
mod monitor;
mod objects;
mod pipeline;
mod snapshot;

use std::env;
use std::process::ExitCode;

fn usage() {
    eprintln!(
        "llsm-replay — replayable low-latency streaming monitor experiment\n\
         \n\
         USAGE:\n\
         \x20 llsm-replay run                  run the full experiment, keep evidence, clean temp state\n\
         \x20 llsm-replay replay <journal>     re-execute a journal and compare result digests\n\
         \x20 llsm-replay monitor              open the GTK4 low-latency monitor window\n\
         \x20 llsm-replay clean                remove leftover temporary state"
    );
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => experiment::run(),
        Some("replay") => match args.get(1) {
            Some(path) => experiment::replay(path),
            None => {
                usage();
                return ExitCode::from(2);
            }
        },
        Some("monitor") => monitor::run(),
        Some("clean") => experiment::clean(),
        _ => {
            usage();
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
