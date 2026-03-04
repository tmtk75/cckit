#[cfg(target_os = "macos")]
fn main() {
    if let Err(e) = cckit::monitor::window::run_window_app() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("cckit-window is only supported on macOS");
    std::process::exit(1);
}
