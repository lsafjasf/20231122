fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1).is_some_and(|arg| arg == "headless") {
        if let Err(error) = hdr_repair::cli::run_headless(&args[2..]) {
            eprintln!("headless run failed: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(feature = "gui")]
    {
        if args.len() > 1 && matches!(args[1].as_str(), "--help" | "-h") {
            println!(
                "hdr-repair                         launch GTK4 workbench\n\
                 hdr-repair headless [OPTIONS]     run the same repair pipeline without GUI"
            );
            return;
        }
        std::process::exit(hdr_repair::ui::run().get() as i32);
    }
    #[cfg(not(feature = "gui"))]
    {
        eprintln!("build with the gui feature to launch the GTK4 application");
    }
}
