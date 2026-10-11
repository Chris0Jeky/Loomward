//! `loomward-desktop`: the Tauri 2 shell around the built app (`app/dist`). Two IPC commands,
//! `lw_call` and `lw_events`; the capability file grants nothing else. It hosts the engine service
//! in a personal session by default (`launch.rs` explains the opt-in); `--dataset synthetic` and
//! `--fixtures` are the demo and test modes.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use loomward_desktop::dialogs::DesktopDialogs;
use loomward_desktop::launch::{self, Launch, USAGE};
use loomward_desktop::{selftest, shell, Desktop};
use std::process::ExitCode;
use std::sync::Arc;

/// Console output for `--self-test` from a release build (GUI subsystem has no console).
fn attach_console() {
    #[cfg(windows)]
    // SAFETY: plain Win32 call with a constant argument; failure just leaves no console.
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
}

fn main() -> ExitCode {
    let launch = match Launch::parse(std::env::args().skip(1)) {
        Ok(l) => l,
        Err(e) => {
            attach_console();
            eprintln!("loomward-desktop: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if launch.self_test {
        attach_console();
        let ok = selftest::run(&mut |line| println!("{line}"));
        println!("self-test {}", if ok { "passed" } else { "FAILED" });
        return if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    let service = match launch::open(&launch, std::env::var_os("LOCALAPPDATA")) {
        Ok(s) => s,
        Err(e) => {
            attach_console();
            eprintln!("loomward-desktop: {e}");
            return ExitCode::FAILURE;
        }
    };
    let dialogs = Arc::new(DesktopDialogs::new(launch.dataset));
    let desktop = Arc::new(Desktop::new(service, launch.dataset, dialogs.clone()));
    let result = shell::configure(tauri::Builder::default().manage(dialogs), desktop)
        .run(tauri::generate_context!());
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            attach_console();
            eprintln!("loomward-desktop: {e}");
            ExitCode::FAILURE
        }
    }
}
