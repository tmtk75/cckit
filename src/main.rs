fn main() {
    // When launched from a macOS .app bundle with no arguments, run as app (window + menubar)
    if std::env::args().len() == 1 && is_in_app_bundle() {
        #[cfg(target_os = "macos")]
        {
            if let Err(e) = cckit::monitor::window::run_app(false, false) {
                eprintln!("Error running app: {}", e);
                std::process::exit(1);
            }
            return;
        }
    }

    cckit::cli::run();
}

fn is_in_app_bundle() -> bool {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
        .unwrap_or(false)
}
