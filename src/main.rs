mod analyze;
mod cli;
mod events;
mod evidence;
mod gui;
mod model;
mod overlay;
mod waveform;

fn main() {
    if let Err(error) = cli::run() {
        eprintln!("error: {error:#}");
        std::process::exit(2);
    }
}
