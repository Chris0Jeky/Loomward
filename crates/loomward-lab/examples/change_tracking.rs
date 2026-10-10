//! Explicitly invoked, disposable Windows change-tracking spike; no engine integration.
#[cfg(windows)]
#[path = "change_tracking/windows.rs"]
mod windows;

fn main() {
    #[cfg(windows)]
    if let Err(error) = windows::run() {
        eprintln!("change-tracking: {error}");
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    {
        eprintln!("change-tracking requires Windows; no measurements made");
        std::process::exit(1);
    }
}
