fn main() -> gtk4::glib::ExitCode {
    match media_meta_replay::experiment::Experiment::builtin() {
        Ok(experiment) => media_meta_replay::ui::run(experiment),
        Err(error) => {
            eprintln!("cannot build replay experiment: {error}");
            gtk4::glib::ExitCode::FAILURE
        }
    }
}
