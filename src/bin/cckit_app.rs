#[cfg(target_os = "macos")]
fn main() {
    // If args provided (beyond program name), run as CLI
    if std::env::args().len() > 1 {
        cckit::cli::run();
        return;
    }

    // Menubar-only app entry point
    if let Err(e) = cckit::monitor::menubar::run_menubar_app(500) {
        eprintln!("Error running menubar app: {}", e);
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    // If args provided, run as CLI
    if std::env::args().len() > 1 {
        cckit::cli::run();
        return;
    }

    eprintln!("cckit-app is only supported on macOS");
    std::process::exit(1);
}
